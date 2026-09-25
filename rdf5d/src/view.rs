//! Scoped, queryable view over a subset of named graphs in a [`Snapshot`].
//!
//! A [`View`] wraps a [`Snapshot`] together with an explicit list of physical
//! graph groups (gids) that form the view's scope. Permutation indexes and
//! the transitive-closure table are built **on demand, from the view's gids
//! only** — not from the entire store. This gives two benefits:
//!
//! 1. **Correct per-view transitive closure** — `subClassOf` chains that
//!    cross named graph boundaries are only followed when both endpoints
//!    live inside the view. Store-wide PClos can silently leak results from
//!    unrelated graphs into a view's SPARQL results.
//! 2. **Lower memory** — Indexes are proportional to the view's triple
//!    count, not the store's. Create as many views as you like; only the
//!    ones actually queried build their indexes.
//!
//! Create a view from named graphs and query it directly:
//!
//! ```ignore
//! let snap = Snapshot::open(&path)?;
//! let view = View::from_names(&snap, &["http://ex/g/A", "http://ex/g/B"]);
//!
//! // SPARQL query scoped to the view
//! let mut query = SparqlParser::new().parse_query("SELECT ?x WHERE { ... }")?;
//! let results = view.query(&mut query)?;
//!
//! // Pattern scan scoped to the view
//! let matches = view.scan(Pattern { s: Some(id), .. });
//! ```

use std::collections::HashSet;
use std::iter::{empty, once};
use std::sync::OnceLock;

use crate::index::{
    IdxKind, MemPClos, MemSection, build_mem_pclos_for_gids, build_mem_section_for_gids,
};
use crate::reader::{DecodedTerm, Result};
use crate::snapshot::{CLOSURE_PREDICATE_IRIS, Match, Pattern, Snapshot};

#[cfg(feature = "sparql")]
use crate::SparqlView;
#[cfg(feature = "sparql")]
use spareval;
#[cfg(feature = "sparql")]
use spargebra;

/// A scoped, lazily-indexed view over a subset of a [`Snapshot`]'s named graphs.
///
/// Cheap to create (just resolves names to gids). Indexes are built on first
/// scan or SPARQL query and cover only the view's gids.
#[derive(Debug)]
pub struct View<'a> {
    snapshot: &'a Snapshot,
    gids: Vec<u64>,
    // Permutation indexes, built lazily and independently from the view's
    // gids only. `None` inside the cell means the build failed (logged).
    mem_pso: OnceLock<Option<MemSection>>,
    mem_pos: OnceLock<Option<MemSection>>,
    mem_spo: OnceLock<Option<MemSection>>,
    mem_osp: OnceLock<Option<MemSection>>,
    // Precomputed transitive closures for CLOSURE_PREDICATE_IRIS, computed
    // over triples in the view's gids only.
    mem_pclos: OnceLock<Option<MemPClos>>,
}

impl<'a> View<'a> {
    /// Create a view from a list of logical-graph names.
    ///
    /// Resolves each name to its physical gids. Names not present in the
    /// snapshot are silently skipped.
    pub fn from_names(snapshot: &'a Snapshot, names: &[&str]) -> Self {
        let mut all_gids = Vec::new();
        for name in names {
            if let Some(gids) = snapshot.gids_for_name(name) {
                all_gids.extend_from_slice(gids);
            }
        }
        all_gids.sort_unstable();
        all_gids.dedup();
        Self {
            snapshot,
            gids: all_gids,
            mem_pso: OnceLock::new(),
            mem_pos: OnceLock::new(),
            mem_spo: OnceLock::new(),
            mem_osp: OnceLock::new(),
            mem_pclos: OnceLock::new(),
        }
    }

    /// Create a view from an explicit list of physical gids.
    pub fn from_gids(snapshot: &'a Snapshot, mut gids: Vec<u64>) -> Self {
        gids.sort_unstable();
        gids.dedup();
        Self {
            snapshot,
            gids,
            mem_pso: OnceLock::new(),
            mem_pos: OnceLock::new(),
            mem_spo: OnceLock::new(),
            mem_osp: OnceLock::new(),
            mem_pclos: OnceLock::new(),
        }
    }

    /// The gids comprising this view.
    pub fn gids(&self) -> &[u64] {
        &self.gids
    }

    #[cfg(feature = "sparql")]
    pub(crate) fn snapshot(&self) -> &Snapshot {
        self.snapshot
    }

    /// Number of physical gids in the view.
    pub fn n_gids(&self) -> usize {
        self.gids.len()
    }

    /// Stream `(gid, s, p, o)` matches for `pat` within the view.
    ///
    /// Multi-gid views deduplicate by triple. Uses the view's own permutation
    /// indexes when a term is bound and the relevant index is available;
    /// otherwise falls back to a per-gid scan of the view's gids.
    pub fn scan(&'a self, pat: Pattern) -> Box<dyn Iterator<Item = Result<Match>> + 'a> {
        self.scan_gids(pat, self.gids.clone())
    }

    pub(crate) fn scan_gids(
        &'a self,
        pat: Pattern,
        gids: Vec<u64>,
    ) -> Box<dyn Iterator<Item = Result<Match>> + 'a> {
        let n_terms = self.snapshot.file().num_terms();
        if [pat.s, pat.p, pat.o]
            .iter()
            .any(|bound| bound.is_some_and(|id| id >= n_terms))
        {
            return Box::new(empty());
        }

        let dedup = gids.len() > 1;

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

    /// A SPARQL dataset view over this view's gids.
    ///
    /// The returned `SparqlView` sees only the named graphs in this view and
    /// deduplicates triples across gids that share a name.
    #[cfg(feature = "sparql")]
    pub fn sparql_view(&'a self) -> SparqlView<'a> {
        SparqlView::scoped(self)
    }

    /// Rewrite and evaluate a SPARQL query against this view.
    #[cfg(feature = "sparql")]
    pub fn query(
        &'a self,
        query: &'a mut spargebra::Query,
    ) -> std::result::Result<spareval::QueryResults<'a>, spareval::QueryEvaluationError> {
        self.rewrite_query(query);
        spareval::QueryEvaluator::new()
            .prepare(query)
            .execute(self.sparql_view())
    }

    /// Rewrite `P+`/`P*` property paths using the view's PClos.
    #[cfg(feature = "sparql")]
    pub fn rewrite_query(&self, query: &mut spargebra::Query) {
        crate::sparql::PClosRewriter::for_view(self).rewrite_query(query);
    }

    /// The dedup-aware unique-triple count for this view.
    ///
    /// Each call scans the selected gids.
    pub fn triple_count(&self) -> Result<usize> {
        count_unique(self.snapshot.file(), &self.gids)
    }

    /// Whether the view contains a specific triple.
    pub fn contains(&self, s: Option<u64>, p: Option<u64>, o: Option<u64>) -> Result<bool> {
        let pat = Pattern { s, p, o };
        if let Some(hit) = self.scan(pat).next() {
            hit?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Forward closure for a predicate, scoped to this view.
    pub fn closure_forward(&self, predicate: u64, subject: u64) -> Option<Vec<u64>> {
        self.mem_pclos()?.closure_forward(predicate, subject)
    }

    /// Reverse closure for a predicate, scoped to this view.
    pub fn closure_reverse(&self, predicate: u64, object: u64) -> Option<Vec<u64>> {
        self.mem_pclos()?.closure_reverse(predicate, object)
    }

    #[cfg(feature = "sparql")]
    pub(crate) fn has_closures(&self) -> bool {
        self.mem_pclos().is_some()
    }

    // ── index building (lazy, per-view) ─────────────────────────────────

    fn mem_section(&self, kind: IdxKind) -> Option<&MemSection> {
        let cell = match kind {
            IdxKind::Pso => &self.mem_pso,
            IdxKind::Pos => &self.mem_pos,
            IdxKind::Spo => &self.mem_spo,
            IdxKind::Osp => &self.mem_osp,
            IdxKind::PClos => return None,
        };
        cell.get_or_init(|| {
            match build_mem_section_for_gids(self.snapshot.file(), kind, &self.gids) {
                Ok(section) => Some(section),
                Err(error) => {
                    log::warn!("building {kind:?} index for view failed: {error}");
                    None
                }
            }
        })
        .as_ref()
    }

    fn mem_pclos(&self) -> Option<&MemPClos> {
        self.mem_pclos
            .get_or_init(|| {
                let mut pred_ids = Vec::new();
                for iri in CLOSURE_PREDICATE_IRIS {
                    let term = DecodedTerm::Iri(std::borrow::Cow::Borrowed(iri));
                    if let Some(id) = self.snapshot.file().term_id(&term) {
                        pred_ids.push(id);
                    }
                }
                if pred_ids.is_empty() {
                    return None;
                }
                match build_mem_pclos_for_gids(self.snapshot.file(), &pred_ids, &self.gids) {
                    Ok(pclos) => Some(pclos),
                    Err(error) => {
                        log::warn!("building view closure index failed: {error}");
                        None
                    }
                }
            })
            .as_ref()
    }

    // ── scan internals ──────────────────────────────────────────────────

    fn indexed_iter(
        &'a self,
        pat: Pattern,
        gids: Vec<u64>,
    ) -> std::result::Result<Box<dyn Iterator<Item = Result<Match>> + 'a>, Vec<u64>> {
        crate::snapshot::indexed_iter_with_sections(pat, gids, |kind| self.mem_section(kind))
    }

    fn full_scan(
        &'a self,
        pat: Pattern,
        gids: Vec<u64>,
    ) -> impl Iterator<Item = Result<Match>> + 'a {
        let file = self.snapshot.file();
        gids.into_iter()
            .flat_map(move |gid| match file.triples_ids_lazy(gid) {
                Ok(triples) => {
                    let pat = pat;
                    Box::new(triples.filter_map(move |triple| match triple {
                        Err(error) => Some(Err(error)),
                        Ok((s, p, o))
                            if pat.s.is_none_or(|x| x == s)
                                && pat.p.is_none_or(|x| x == p)
                                && pat.o.is_none_or(|x| x == o) =>
                        {
                            Some(Ok(Match { gid, s, p, o }))
                        }
                        Ok(_) => None,
                    })) as Box<dyn Iterator<Item = Result<Match>> + 'a>
                }
                Err(error) => Box::new(once(Err(error))),
            })
    }
}

/// Wraps a match iterator, dropping triples already seen.
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

fn count_unique(file: &crate::reader::R5tuFile, gids: &[u64]) -> crate::reader::Result<usize> {
    let mut triples = HashSet::new();
    for &gid in gids {
        for triple in file.triples_ids(gid)? {
            triples.insert(triple);
        }
    }
    Ok(triples.len())
}
