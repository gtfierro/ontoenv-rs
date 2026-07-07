//! Read-only query snapshot over an [`R5tuFile`].
//!
//! A [`Snapshot`] wraps an opened `.r5tu` file together with the derived state
//! a query workload needs: the grouping of physical graph groups (gids) by
//! graph **name** (the "logical graph" view), and the lazily-built permutation
//! indexes. Because a snapshot is immutable for its lifetime, every piece of
//! derived state is built at most once and never invalidated.
//!
//! The single access primitive is [`Snapshot::scan`], which streams the
//! `(gid, s, p, o)` term-id tuples matching a [`Pattern`] within a [`Scope`].
//! It picks an index lookup when a term is bound and an index is available, and
//! otherwise falls back to a per-graph scan. SPARQL evaluation
//! ([`crate::sparql`]) and the higher-level store APIs are both expressed on
//! top of it, so the "index-or-scan" decision lives in exactly one place.

use std::collections::{HashMap, HashSet};
use std::iter::{empty, once};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::index::{IdxKind, MemPClos, MemSection, build_mem_pclos, build_mem_section};
use crate::reader::{DecodedTerm, R5tuFile, Result};

/// Predicate IRIs whose transitive closure is precomputed (in memory, on first
/// use) to accelerate SPARQL `P+`/`P*` property paths.
pub const CLOSURE_PREDICATE_IRIS: &[&str] = &[
    "http://www.w3.org/2000/01/rdf-schema#subClassOf",
    "http://www.w3.org/2000/01/rdf-schema#subPropertyOf",
    "http://www.w3.org/2002/07/owl#sameAs",
];

/// A triple pattern over on-disk term ids. `None` in a slot is a wildcard.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pattern {
    pub s: Option<u64>,
    pub p: Option<u64>,
    pub o: Option<u64>,
}

impl Pattern {
    /// The all-wildcard pattern (matches every triple).
    pub const ANY: Pattern = Pattern {
        s: None,
        p: None,
        o: None,
    };
}

/// The set of graphs a [`Snapshot::scan`] ranges over.
///
/// [`Scope::ByName`] deduplicates by triple across the physical gids that share
/// the name (so a logical graph is a single named graph); [`Scope::All`] and
/// [`Scope::Gids`] do not deduplicate.
#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    /// Every physical graph group in the snapshot.
    All,
    /// An explicit set of physical graph ids.
    Gids(&'a [u64]),
    /// All gids sharing one graph name, presented as one logical graph.
    ByName(&'a str),
}

/// One `(gid, s, p, o)` triple match, all as on-disk term ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Match {
    pub gid: u64,
    pub s: u64,
    pub p: u64,
    pub o: u64,
}

/// One logical (by-name) graph: the physical gids sharing the name, plus a
/// lazily-computed dedup-aware unique-triple count.
#[derive(Debug)]
struct LogicalGraph {
    gids: Vec<u64>,
    /// Exact, dedup-aware unique-triple count, filled on first request. For a
    /// single-gid logical graph it is initialized at open time from the GDIR
    /// `n_triples`, since no cross-gid dedup is needed.
    triple_count: OnceLock<usize>,
}

/// A read-only, queryable view over an opened `.r5tu` file.
#[derive(Debug)]
pub struct Snapshot {
    file: Arc<R5tuFile>,
    by_name: HashMap<String, LogicalGraph>,
    // Permutation indexes, built in memory from `file` on first use. Each is
    // independent so we only pay for the permutations a workload queries.
    // `None` inside the cell means the build failed (logged).
    mem_pso: OnceLock<Option<MemSection>>,
    mem_pos: OnceLock<Option<MemSection>>,
    mem_spo: OnceLock<Option<MemSection>>,
    mem_osp: OnceLock<Option<MemSection>>,
    // Precomputed transitive closures for CLOSURE_PREDICATE_IRIS.
    mem_pclos: OnceLock<Option<MemPClos>>,
}

impl Snapshot {
    /// Open a `.r5tu` file as a query snapshot. Uses an mmap-backed reader when
    /// the `mmap` feature is enabled, falling back to an owned read otherwise.
    pub fn open(path: &Path) -> Result<Self> {
        #[cfg(feature = "mmap")]
        let file = R5tuFile::open_mmap(path)?;
        #[cfg(not(feature = "mmap"))]
        let file = R5tuFile::open(path)?;
        Self::from_file(Arc::new(file))
    }

    /// Build a snapshot from an already-opened file. Groups the file's physical
    /// graphs by name to form the logical-graph view.
    pub fn from_file(file: Arc<R5tuFile>) -> Result<Self> {
        let mut grouped: HashMap<String, Vec<(u64, u64)>> = HashMap::new();
        for graph in file.enumerate_all()? {
            grouped
                .entry(graph.graphname)
                .or_default()
                .push((graph.gid, graph.n_triples));
        }

        let mut by_name = HashMap::with_capacity(grouped.len());
        for (name, entries) in grouped {
            let triple_count = OnceLock::new();
            if entries.len() == 1 {
                // Single physical gid: no cross-gid dedup needed.
                let _ = triple_count.set(entries[0].1 as usize);
            }
            let gids = entries.into_iter().map(|(gid, _)| gid).collect();
            by_name.insert(name, LogicalGraph { gids, triple_count });
        }

        Ok(Self {
            file,
            by_name,
            mem_pso: OnceLock::new(),
            mem_pos: OnceLock::new(),
            mem_spo: OnceLock::new(),
            mem_osp: OnceLock::new(),
            mem_pclos: OnceLock::new(),
        })
    }

    /// The underlying file reader.
    pub fn file(&self) -> &R5tuFile {
        &self.file
    }

    /// The distinct logical-graph names in this snapshot.
    pub fn graph_names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    /// Eagerly build all four permutation indexes (PSO, POS, SPO, OSP) so the
    /// cost is paid up front at bind time rather than billed to the first
    /// query of each shape. Each index is built in its own thread. Idempotent:
    /// already-built indexes are returned from the `OnceLock` unchanged.
    ///
    /// Build failures are logged and the index left `None`, matching the
    /// lazy path's behavior.
    pub fn build_indexes(&self) {
        // Build the reverse term-id index first; the permutation builders and
        // term-id resolution both depend on it.
        self.file.build_term_index();
        std::thread::scope(|s| {
            let _ = s.spawn(|| self.mem_section(IdxKind::Pso));
            let _ = s.spawn(|| self.mem_section(IdxKind::Pos));
            let _ = s.spawn(|| self.mem_section(IdxKind::Spo));
            let _ = s.spawn(|| self.mem_section(IdxKind::Osp));
        });
    }

    /// Whether a logical graph with this name exists.
    pub fn has_graph(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// The physical gids backing a logical-graph name, if any.
    pub fn gids_for_name(&self, name: &str) -> Option<&[u64]> {
        self.by_name.get(name).map(|info| info.gids.as_slice())
    }

    /// Stream the `(gid, s, p, o)` matches for `pat` within `scope`.
    ///
    /// Uses a permutation index when a term is bound and the relevant index is
    /// available, otherwise a per-graph scan. `Scope::ByName` spanning more than
    /// one gid deduplicates by triple.
    pub fn scan<'a>(
        &'a self,
        pat: Pattern,
        scope: Scope<'_>,
    ) -> Box<dyn Iterator<Item = Result<Match>> + 'a> {
        let (gids, dedup) = match scope {
            Scope::All => (self.all_gids(), false),
            Scope::Gids(gids) => (gids.to_vec(), false),
            Scope::ByName(name) => match self.by_name.get(name) {
                Some(info) => (info.gids.clone(), info.gids.len() > 1),
                None => return Box::new(empty()),
            },
        };

        // A bound term absent from the on-disk dictionary was interned to an
        // overflow id (>= num_terms) that can never equal a scanned id, so the
        // pattern matches nothing.
        let n_terms = self.file.num_terms();
        if [pat.s, pat.p, pat.o]
            .iter()
            .any(|bound| bound.is_some_and(|id| id >= n_terms))
        {
            return Box::new(empty());
        }

        let base: Box<dyn Iterator<Item = Result<Match>> + 'a> = match self.indexed_iter(pat, gids)
        {
            Ok(iter) => iter,
            Err(gids) => Box::new(self.full_scan(pat, gids)),
        };

        if dedup {
            Box::new(Dedup {
                inner: base,
                seen: HashSet::new(),
            })
        } else {
            base
        }
    }

    /// The dedup-aware unique-triple count for a scope. `ByName` is cached.
    pub fn triple_count(&self, scope: Scope<'_>) -> Result<usize> {
        match scope {
            Scope::ByName(name) => {
                let Some(info) = self.by_name.get(name) else {
                    return Ok(0);
                };
                if let Some(count) = info.triple_count.get() {
                    return Ok(*count);
                }
                let count = count_unique(&self.file, &info.gids)?;
                let _ = info.triple_count.set(count);
                Ok(count)
            }
            Scope::Gids(gids) => count_unique(&self.file, gids),
            Scope::All => count_unique(&self.file, &self.all_gids()),
        }
    }

    /// The logical-graph names whose triples include a match for `pat`.
    pub fn names_containing(&self, pat: Pattern) -> Result<Vec<&str>> {
        let mut out = Vec::new();
        for name in self.by_name.keys() {
            match self.scan(pat, Scope::ByName(name)).next() {
                Some(Ok(_)) => out.push(name.as_str()),
                Some(Err(error)) => return Err(error),
                None => {}
            }
        }
        Ok(out)
    }

    /// Forward closure: objects reachable from `subject` via one or more
    /// `predicate` edges. `None` when `predicate` has no precomputed closure.
    pub fn closure_forward(&self, predicate: u64, subject: u64) -> Option<Vec<u64>> {
        self.mem_pclos()?.closure_forward(predicate, subject)
    }

    /// Reverse closure: subjects that reach `object` via one or more
    /// `predicate` edges.
    pub fn closure_reverse(&self, predicate: u64, object: u64) -> Option<Vec<u64>> {
        self.mem_pclos()?.closure_reverse(predicate, object)
    }

    /// Whether any precomputed closure data is available.
    pub fn has_closures(&self) -> bool {
        self.mem_pclos().is_some()
    }

    // ---- internals ----

    fn all_gids(&self) -> Vec<u64> {
        let mut gids = Vec::new();
        for info in self.by_name.values() {
            gids.extend_from_slice(&info.gids);
        }
        gids
    }

    fn full_scan<'a>(
        &'a self,
        pat: Pattern,
        gids: Vec<u64>,
    ) -> impl Iterator<Item = Result<Match>> + 'a {
        let file: &'a R5tuFile = self.file.as_ref();
        gids.into_iter()
            .flat_map(move |gid| -> Box<dyn Iterator<Item = Result<Match>> + 'a> {
                match file.triples_ids(gid) {
                    Ok(triples) => Box::new(triples.filter_map(move |(s, p, o)| {
                        if pat.s.is_some_and(|x| x != s)
                            || pat.p.is_some_and(|x| x != p)
                            || pat.o.is_some_and(|x| x != o)
                        {
                            None
                        } else {
                            Some(Ok(Match { gid, s, p, o }))
                        }
                    })),
                    Err(error) => Box::new(once(Err(error))),
                }
            })
    }

    /// Serve a bound-term pattern from the permutation indexes as a streaming
    /// iterator. The returned iterator borrows the snapshot's lazily-built
    /// index and owns `gids`.
    ///
    /// Returns `Err(gids)` (handing ownership back) when no term is bound (no
    /// index applies) or the relevant index/posting is absent, so the caller
    /// can fall back to a full scan without re-deriving the gid set.
    #[allow(clippy::type_complexity)]
    fn indexed_iter<'a>(
        &'a self,
        pat: Pattern,
        gids: Vec<u64>,
    ) -> std::result::Result<Box<dyn Iterator<Item = Result<Match>> + 'a>, Vec<u64>> {
        match (pat.s, pat.p, pat.o) {
            // Predicate + object bound: POS (pairs are (object, subject)).
            (subject, Some(p_id), Some(o_id)) => {
                let Some(post) = self.mem_section(IdxKind::Pos).and_then(|s| s.lookup(p_id)) else {
                    return Err(gids);
                };
                Ok(Box::new(gids.into_iter().flat_map(move |gid| {
                    post.block_for_gid(gid).into_iter().flat_map(move |block| {
                        post.iter_block(block).filter_map(move |(o, s)| {
                            if o != o_id || subject.is_some_and(|sid| s != sid) {
                                None
                            } else {
                                Some(Ok(Match { gid, s, p: p_id, o }))
                            }
                        })
                    })
                })))
            }
            // Predicate bound, object unbound: PSO (pairs are (subject, object)).
            (subject, Some(p_id), None) => {
                let Some(post) = self.mem_section(IdxKind::Pso).and_then(|s| s.lookup(p_id)) else {
                    return Err(gids);
                };
                Ok(Box::new(gids.into_iter().flat_map(move |gid| {
                    post.block_for_gid(gid).into_iter().flat_map(move |block| {
                        post.iter_block(block).filter_map(move |(s, o)| {
                            if subject.is_some_and(|sid| s != sid) {
                                None
                            } else {
                                Some(Ok(Match { gid, s, p: p_id, o }))
                            }
                        })
                    })
                })))
            }
            // Subject bound, predicate unbound: SPO (pairs are (predicate, object)).
            (Some(s_id), None, object) => {
                let Some(post) = self.mem_section(IdxKind::Spo).and_then(|s| s.lookup(s_id)) else {
                    return Err(gids);
                };
                Ok(Box::new(gids.into_iter().flat_map(move |gid| {
                    post.block_for_gid(gid).into_iter().flat_map(move |block| {
                        post.iter_block(block).filter_map(move |(p, o)| {
                            if object.is_some_and(|oid| o != oid) {
                                None
                            } else {
                                Some(Ok(Match { gid, s: s_id, p, o }))
                            }
                        })
                    })
                })))
            }
            // Object bound, subject+predicate unbound: OSP (pairs are (subject, predicate)).
            (None, None, Some(o_id)) => {
                let Some(post) = self.mem_section(IdxKind::Osp).and_then(|s| s.lookup(o_id)) else {
                    return Err(gids);
                };
                Ok(Box::new(gids.into_iter().flat_map(move |gid| {
                    post.block_for_gid(gid).into_iter().flat_map(move |block| {
                        post.iter_block(block)
                            .map(move |(s, p)| Ok(Match { gid, s, p, o: o_id }))
                    })
                })))
            }
            // All unbound: no index can help — caller does a full scan.
            (None, None, None) => Err(gids),
        }
    }

    /// Lazily build (once) and borrow a permutation index, or `None` on failure.
    fn mem_section(&self, kind: IdxKind) -> Option<&MemSection> {
        let cell = match kind {
            IdxKind::Pso => &self.mem_pso,
            IdxKind::Pos => &self.mem_pos,
            IdxKind::Spo => &self.mem_spo,
            IdxKind::Osp => &self.mem_osp,
            IdxKind::PClos => return None,
        };
        cell.get_or_init(|| match build_mem_section(&self.file, kind) {
            Ok(section) => Some(section),
            Err(error) => {
                log::warn!("building {kind:?} index failed: {error}");
                None
            }
        })
        .as_ref()
    }

    /// Lazily build (once) and borrow the precomputed transitive-closure index.
    fn mem_pclos(&self) -> Option<&MemPClos> {
        self.mem_pclos
            .get_or_init(|| {
                // Resolve the closure-predicate IRIs to ids present in this
                // snapshot; skip any that are absent.
                let mut pred_ids = Vec::new();
                for iri in CLOSURE_PREDICATE_IRIS {
                    let term = DecodedTerm::Iri(std::borrow::Cow::Borrowed(iri));
                    if let Some(id) = self.file.term_id(&term) {
                        pred_ids.push(id);
                    }
                }
                if pred_ids.is_empty() {
                    return None;
                }
                match build_mem_pclos(&self.file, &pred_ids) {
                    Ok(pclos) => Some(pclos),
                    Err(error) => {
                        log::warn!("building closure index failed: {error}");
                        None
                    }
                }
            })
            .as_ref()
    }
}

/// Wraps a match iterator, dropping triples already seen (by `(s, p, o)`).
struct Dedup<'a> {
    inner: Box<dyn Iterator<Item = Result<Match>> + 'a>,
    seen: HashSet<(u64, u64, u64)>,
}

impl Iterator for Dedup<'_> {
    type Item = Result<Match>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next()? {
                Ok(m) => {
                    if self.seen.insert((m.s, m.p, m.o)) {
                        return Some(Ok(m));
                    }
                }
                Err(error) => return Some(Err(error)),
            }
        }
    }
}

fn count_unique(file: &R5tuFile, gids: &[u64]) -> Result<usize> {
    let mut triples = HashSet::new();
    for &gid in gids {
        for triple in file.triples_ids(gid)? {
            triples.insert(triple);
        }
    }
    Ok(triples.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Quint, Term, write_file};
    use std::borrow::Cow;
    use tempfile::tempdir;

    fn iri(s: &str) -> Term {
        Term::Iri(s.into())
    }

    fn literal(s: &str) -> Term {
        Term::Literal {
            lex: s.into(),
            dt: None,
            lang: None,
        }
    }

    /// Two gids sharing one graph name, plus a third gid under a different name.
    /// g1: (alice, name, "Alice") and (alice, age, "30")
    /// g2: (bob,   name, "Bob")  and (charlie, name, "Charlie") — under same name
    /// g3: (dave,  name, "Dave") under a different name
    fn multi_gid_fixture(path: &Path) {
        let quints = vec![
            Quint {
                id: "d1".into(),
                s: iri("http://ex/alice"),
                p: iri("http://ex/name"),
                o: literal("Alice"),
                gname: "http://ex/g/shared".into(),
            },
            Quint {
                id: "d1".into(),
                s: iri("http://ex/alice"),
                p: iri("http://ex/age"),
                o: literal("30"),
                gname: "http://ex/g/shared".into(),
            },
            Quint {
                id: "d2".into(),
                s: iri("http://ex/bob"),
                p: iri("http://ex/name"),
                o: literal("Bob"),
                gname: "http://ex/g/shared".into(),
            },
            Quint {
                id: "d2".into(),
                s: iri("http://ex/charlie"),
                p: iri("http://ex/name"),
                o: literal("Charlie"),
                gname: "http://ex/g/shared".into(),
            },
            Quint {
                id: "d3".into(),
                s: iri("http://ex/dave"),
                p: iri("http://ex/name"),
                o: literal("Dave"),
                gname: "http://ex/g/other".into(),
            },
        ];
        write_file(path, &quints).expect("multi-gid fixture");
    }

    fn snap(path: &Path) -> Snapshot {
        Snapshot::open(path).expect("open snapshot")
    }

    fn term_id(snap: &Snapshot, iri_str: &str) -> u64 {
        snap.file()
            .term_id(&DecodedTerm::Iri(Cow::Borrowed(iri_str)))
            .unwrap_or_else(|| panic!("term not found: {iri_str}"))
    }

    fn count_matches(results: &[Result<Match>]) -> usize {
        results.iter().filter(|r| r.is_ok()).count()
    }

    #[test]
    fn empty_snapshot_returns_no_graphs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.r5tu");
        write_file(&path, &[]).expect("empty fixture");
        let s = snap(&path);
        assert_eq!(s.graph_names().count(), 0);
        assert!(!s.has_graph("http://ex/g/shared"));
        assert_eq!(s.gids_for_name("http://ex/g/shared"), None);
    }

    #[test]
    fn scan_all_returns_all_triples() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("scan_all.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        let results: Vec<_> = s.scan(Pattern::ANY, Scope::All).collect();
        assert_eq!(count_matches(&results), 5);
    }

    #[test]
    fn scan_by_name_single_gid() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("scan_single.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        // "other" has 1 triple under a single gid
        let results: Vec<_> = s
            .scan(Pattern::ANY, Scope::ByName("http://ex/g/other"))
            .collect();
        assert_eq!(count_matches(&results), 1);
        // Check triple content
        let m = results[0].as_ref().unwrap();
        let ns = "http://ex/";
        let dave_id = term_id(&s, &format!("{ns}dave"));
        let name_id = term_id(&s, &format!("{ns}name"));
        assert_eq!(m.s, dave_id);
        assert_eq!(m.p, name_id);
    }

    #[test]
    fn scan_by_name_dedups_across_multi_gid() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("scan_dedup.r5tu");
        // Write the same triple under two gids sharing one name
        let quints = vec![
            Quint {
                id: "d1".into(),
                s: iri("http://ex/alice"),
                p: iri("http://ex/name"),
                o: literal("Alice"),
                gname: "http://ex/g/shared".into(),
            },
            Quint {
                id: "d2".into(),
                s: iri("http://ex/alice"),
                p: iri("http://ex/name"),
                o: literal("Alice"),
                gname: "http://ex/g/shared".into(),
            },
        ];
        write_file(&path, &quints).expect("dedup fixture");
        let s = snap(&path);
        let results: Vec<_> = s
            .scan(Pattern::ANY, Scope::ByName("http://ex/g/shared"))
            .collect();
        // Even though 2 physical gids, the dedup-aware view should surface 1 triple
        assert_eq!(count_matches(&results), 1);
    }

    #[test]
    fn scan_by_gids_does_not_dedup() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("scan_gids.r5tu");
        let quints = vec![
            Quint {
                id: "d1".into(),
                s: iri("http://ex/alice"),
                p: iri("http://ex/name"),
                o: literal("Alice"),
                gname: "http://ex/g/shared".into(),
            },
            Quint {
                id: "d2".into(),
                s: iri("http://ex/alice"),
                p: iri("http://ex/name"),
                o: literal("Alice"),
                gname: "http://ex/g/shared".into(),
            },
        ];
        write_file(&path, &quints).expect("gids fixture");
        let s = snap(&path);
        // Scope::Gids does NOT deduplicate
        let gids: Vec<u64> = s.gids_for_name("http://ex/g/shared").unwrap().to_vec();
        assert_eq!(gids.len(), 2);
        let results: Vec<_> = s.scan(Pattern::ANY, Scope::Gids(&gids)).collect();
        assert_eq!(count_matches(&results), 2);
    }

    #[test]
    fn scan_bound_pattern_matches() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("scan_bound.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        let alice_id = term_id(&s, "http://ex/alice");
        let name_id = term_id(&s, "http://ex/name");

        // Bind subject + predicate
        let pat = Pattern {
            s: Some(alice_id),
            p: Some(name_id),
            o: None,
        };
        let results: Vec<_> = s.scan(pat, Scope::ByName("http://ex/g/shared")).collect();
        assert_eq!(count_matches(&results), 1, "Alice has one name triple");
        assert_eq!(results[0].as_ref().unwrap().s, alice_id);
        assert_eq!(results[0].as_ref().unwrap().p, name_id);
    }

    #[test]
    fn scan_overflow_term_id_yields_no_results() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("scan_overflow.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        let n_terms = s.file().num_terms();
        // An overflow id >= num_terms cannot match any stored term
        let pat = Pattern {
            s: Some(n_terms + 1),
            p: None,
            o: None,
        };
        let results: Vec<_> = s.scan(pat, Scope::All).collect();
        assert_eq!(count_matches(&results), 0);
    }

    #[test]
    fn scan_nonexistent_graph_name_returns_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("scan_nonexist.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        let results: Vec<_> = s
            .scan(Pattern::ANY, Scope::ByName("http://ex/g/nonexistent"))
            .collect();
        assert_eq!(count_matches(&results), 0);
    }

    #[test]
    fn triple_count_single_gid_eager() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("count_single.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        // "other" has exactly 1 gid with 1 triple
        let count = s.triple_count(Scope::ByName("http://ex/g/other")).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn triple_count_multi_gid_dedup_lazy() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("count_dedup.r5tu");
        // Same triple under 2 gids sharing one name
        let quints = vec![
            Quint {
                id: "d1".into(),
                s: iri("http://ex/alice"),
                p: iri("http://ex/name"),
                o: literal("Alice"),
                gname: "http://ex/g/shared".into(),
            },
            Quint {
                id: "d2".into(),
                s: iri("http://ex/alice"),
                p: iri("http://ex/name"),
                o: literal("Alice"),
                gname: "http://ex/g/shared".into(),
            },
            Quint {
                id: "d3".into(),
                s: iri("http://ex/bob"),
                p: iri("http://ex/name"),
                o: literal("Bob"),
                gname: "http://ex/g/shared".into(),
            },
        ];
        write_file(&path, &quints).expect("count dedup fixture");
        let s = snap(&path);
        // 3 physical triples, but 2 unique: (alice, name, Alice) appears twice
        let count = s.triple_count(Scope::ByName("http://ex/g/shared")).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn triple_count_all_scope() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("count_all.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        // Scope::All = all gids, no dedup: 5 physical triples
        let count = s.triple_count(Scope::All).unwrap();
        assert_eq!(count, 5);
    }

    #[test]
    fn triple_count_gids_scope() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("count_gids.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        let gids = s.gids_for_name("http://ex/g/shared").unwrap();
        let count = s.triple_count(Scope::Gids(gids)).unwrap();
        assert_eq!(count, 4, "shared has 4 physical triples across 2 gids");
    }

    #[test]
    fn names_containing_finds_matching_graphs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("names_containing.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        let alice_id = term_id(&s, "http://ex/alice");
        let pat = Pattern {
            s: Some(alice_id),
            p: None,
            o: None,
        };
        let mut names = s.names_containing(pat).unwrap();
        names.sort();
        assert_eq!(names, vec!["http://ex/g/shared"]);
    }

    #[test]
    fn names_containing_nonexistent_term() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("names_containing_none.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        let n_terms = s.file().num_terms();
        let pat = Pattern {
            s: Some(n_terms + 10),
            p: None,
            o: None,
        };
        let names = s.names_containing(pat).unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn has_graph_returns_true_for_known_names() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("has_graph.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        assert!(s.has_graph("http://ex/g/shared"));
        assert!(s.has_graph("http://ex/g/other"));
        assert!(!s.has_graph("http://ex/g/missing"));
    }

    #[test]
    fn gids_for_name_returns_physical_gids() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("gids_for_name.r5tu");
        multi_gid_fixture(&path);
        let s = snap(&path);
        let gids = s.gids_for_name("http://ex/g/shared").unwrap();
        // Two gids: d1 and d2
        assert_eq!(gids.len(), 2);
        // gids are distinct
        assert_ne!(gids[0], gids[1]);
        assert_eq!(s.gids_for_name("http://ex/g/missing"), None);
    }

    #[test]
    fn closure_forward_reverse_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("closure_snap.r5tu");
        let subclass = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
        let quints = vec![
            Quint {
                id: "d1".into(),
                s: iri("http://ex/A"),
                p: iri(subclass),
                o: iri("http://ex/B"),
                gname: "g".into(),
            },
            Quint {
                id: "d2".into(),
                s: iri("http://ex/B"),
                p: iri(subclass),
                o: iri("http://ex/C"),
                gname: "g".into(),
            },
        ];
        write_file(&path, &quints).expect("closure fixture");
        let s = snap(&path);
        let p_id = term_id(&s, subclass);
        let a_id = term_id(&s, "http://ex/A");
        let c_id = term_id(&s, "http://ex/C");

        // Forward: A subClassOf+ ... should reach C transitively
        let fwd = s.closure_forward(p_id, a_id);
        assert!(fwd.is_some(), "A should have forward closure");
        let mut fwd = fwd.unwrap();
        fwd.sort();
        assert!(fwd.contains(&c_id), "A should reach C via B");

        // Reverse: C is reached by A
        let rev = s.closure_reverse(p_id, c_id);
        assert!(rev.is_some(), "C should have reverse closure");
        assert!(rev.unwrap().contains(&a_id), "C should be reachable from A");

        // A has no reverse (nothing subClassOf A)
        assert!(
            s.closure_reverse(p_id, a_id).is_none()
                || s.closure_reverse(p_id, a_id).unwrap().is_empty()
        );

        // Predicate not in index
        assert!(s.closure_forward(999_999, a_id).is_none());
    }

    #[test]
    fn has_closures_true_only_when_index_has_data() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("has_closures.r5tu");
        // No subClassOf triples — the index will be empty
        let quints = vec![Quint {
            id: "d1".into(),
            s: iri("http://ex/A"),
            p: iri("http://ex/p"),
            o: iri("http://ex/B"),
            gname: "g".into(),
        }];
        write_file(&path, &quints).expect("no-closure fixture");
        let s = snap(&path);
        assert!(
            !s.has_closures(),
            "no closure predicates → has_closures = false"
        );
    }

    #[test]
    fn has_closures_true_after_building() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("has_closures_true.r5tu");
        let subclass = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
        let quints = vec![Quint {
            id: "d1".into(),
            s: iri("http://ex/A"),
            p: iri(subclass),
            o: iri("http://ex/B"),
            gname: "g".into(),
        }];
        write_file(&path, &quints).expect("closure fixture");
        let s = snap(&path);
        // Force building the closure index by querying
        let p_id = term_id(&s, subclass);
        let a_id = term_id(&s, "http://ex/A");
        let fwd = s.closure_forward(p_id, a_id);
        assert!(fwd.is_some());
        assert!(s.has_closures());
    }
}
