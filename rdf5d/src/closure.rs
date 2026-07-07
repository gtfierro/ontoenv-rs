//! Zero-copy *closure* semantics over an rdf5d [`Snapshot`].
//!
//! `ontoenv`'s `copy_closure` materializes the imports closure of an ontology
//! and then post-processes it into a single flattened graph:
//!
//! 1. **strip resolved `owl:imports`** — drop `(?s owl:imports ?o)` whose `?o`
//!    is one of the closure's ontology IRIs (the import is already resolved);
//! 2. **collapse ontology declarations** — drop `(?s rdf:type owl:Ontology)`
//!    for every `?s` other than the root, so the root is the sole declared
//!    ontology, and *add* `(root rdf:type owl:Ontology)` if the root did not
//!    declare itself;
//! 3. **consolidate SHACL prefixes** — rewrite every `(?s sh:prefixes ?o)` to
//!    `(?s sh:prefixes root)`, relocate every non-root `(?s sh:declare ?o)`
//!    onto the root, and de-duplicate declarations by `(sh:prefix,
//!    sh:namespace)`.
//!
//! [`ClosurePatch`] reproduces that result *without materializing a copy*. It
//! records the transform as a small set of precomputed on-disk term ids plus a
//! tiny **patch graph** (`removals` are expressed as a `keep` predicate;
//! `additions` are an explicit triple list). Every read path over the snapshot
//! then presents `dedup(scan − removals) ∪ additions` as a **single flattened
//! graph**, matching `copy_closure`'s set semantics (cross-graph duplicates
//! collapse; SPARQL sees one graph).
//!
//! The module is dependency-free w.r.t. `ontoenv`: the OWL/SHACL IRIs it needs
//! are declared here as string constants.

use std::collections::HashSet;
use std::sync::Arc;

use crate::reader::{DecodedTerm, Result};
use crate::snapshot::{Match, Pattern, Scope, Snapshot};

// ---- vocabulary (kept local so rdf5d has no ontoenv dependency) ----

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL_ONTOLOGY: &str = "http://www.w3.org/2002/07/owl#Ontology";
const OWL_IMPORTS: &str = "http://www.w3.org/2002/07/owl#imports";
const SH_PREFIXES: &str = "http://www.w3.org/ns/shacl#prefixes";
const SH_DECLARE: &str = "http://www.w3.org/ns/shacl#declare";
const SH_PREFIX: &str = "http://www.w3.org/ns/shacl#prefix";
const SH_NAMESPACE: &str = "http://www.w3.org/ns/shacl#namespace";

/// A precomputed, snapshot-specific description of the closure transform.
///
/// All ids are in the snapshot's on-disk term-id space (with any missing terms
/// interned to stable overflow ids), so applying the patch at read time is a
/// few integer comparisons per triple plus a lookup in a small addition set.
#[derive(Debug)]
pub struct ClosurePatch {
    type_id: u64,
    ontology_id: u64,
    imports_id: u64,
    prefixes_id: u64,
    declare_id: u64,
    root_id: u64,

    /// `owl:imports` objects to strip (the closure's ontology IRIs). Empty when
    /// `remove_owl_imports` is false.
    imports_targets: HashSet<u64>,
    remove_owl_imports: bool,

    /// When true, `sh:prefixes`/`sh:declare` are consolidated onto the root:
    /// original `sh:prefixes` triples and non-root `sh:declare` triples are
    /// dropped (see [`Self::keep`]) and re-added via `additions`.
    rewrite_sh_prefixes: bool,
    /// `sh:declare` object nodes relocated onto the root (their identity is
    /// preserved; only the subject changes to the root).
    declare_subjects_moved: HashSet<u64>,

    /// The explicit triples the transform *adds* (patch graph), already in
    /// term-id space and de-duplicated among themselves. Read paths must also
    /// dedup these against the surviving scanned triples.
    additions: Vec<(u64, u64, u64)>,
    /// Fast membership for `additions` (used by `contains`).
    additions_set: HashSet<(u64, u64, u64)>,
}

impl ClosurePatch {
    /// Build the patch from a bound snapshot.
    ///
    /// * `root_iri` — the ontology the closure was computed for (the root onto
    ///   which declarations and SHACL prefixes are collapsed).
    /// * `closure_iris` — every ontology IRI in the resolved closure (the
    ///   second element of `get_closure`'s return tuple); used to decide which
    ///   `owl:imports` are "resolved".
    pub fn build(
        snapshot: &Snapshot,
        root_iri: &str,
        closure_iris: &[String],
        remove_owl_imports: bool,
        rewrite_sh_prefixes: bool,
    ) -> Result<Arc<Self>> {
        let file = snapshot.file();
        let intern = |iri: &str| file.intern_decoded(&DecodedTerm::Iri(iri.into()));

        let type_id = intern(RDF_TYPE);
        let ontology_id = intern(OWL_ONTOLOGY);
        let imports_id = intern(OWL_IMPORTS);
        let prefixes_id = intern(SH_PREFIXES);
        let declare_id = intern(SH_DECLARE);
        let sh_prefix_id = intern(SH_PREFIX);
        let sh_namespace_id = intern(SH_NAMESPACE);
        let root_id = intern(root_iri);

        let imports_targets: HashSet<u64> = if remove_owl_imports {
            closure_iris
                .iter()
                .filter_map(|iri| file.term_id(&DecodedTerm::Iri(iri.as_str().into())))
                .collect()
        } else {
            HashSet::new()
        };

        let mut additions: Vec<(u64, u64, u64)> = Vec::new();
        let mut declare_subjects_moved: HashSet<u64> = HashSet::new();

        // (2) additive root declaration: add `(root a owl:Ontology)` unless the
        // root already declares itself somewhere in the closure.
        let decl_pat = Pattern {
            s: Some(root_id),
            p: Some(type_id),
            o: Some(ontology_id),
        };
        let root_declares_itself = match snapshot.scan(decl_pat, Scope::All).next() {
            Some(Ok(_)) => true,
            Some(Err(e)) => return Err(e),
            None => false,
        };
        if !root_declares_itself {
            additions.push((root_id, type_id, ontology_id));
        }

        // (3) SHACL prefix consolidation. Only computed when requested; the
        // matching removals are handled in `keep`.
        if rewrite_sh_prefixes {
            // sh:prefixes ?o  ->  ?s sh:prefixes root  (dedup by subject).
            let mut prefixes_subjects: HashSet<u64> = HashSet::new();
            let prefixes_pat = Pattern {
                s: None,
                p: Some(prefixes_id),
                o: None,
            };
            for hit in snapshot.scan(prefixes_pat, Scope::All) {
                let m = hit?;
                if prefixes_subjects.insert(m.s) {
                    additions.push((m.s, prefixes_id, root_id));
                }
            }

            // sh:declare relocation onto root, de-duplicated by (prefix, ns).
            // Seed `seen` with declarations already on the root so we don't
            // move a duplicate up.
            let mut seen: HashSet<(String, String)> = HashSet::new();
            let declare_pat = Pattern {
                s: None,
                p: Some(declare_id),
                o: None,
            };
            // First pass: record the root's own declarations.
            for hit in snapshot.scan(declare_pat, Scope::All) {
                let m = hit?;
                if m.s != root_id {
                    continue;
                }
                if let Some(key) = decl_key(snapshot, m.o, sh_prefix_id, sh_namespace_id)? {
                    seen.insert(key);
                }
            }
            // Second pass: relocate non-root declarations, deduping.
            for hit in snapshot.scan(declare_pat, Scope::All) {
                let m = hit?;
                if m.s == root_id {
                    continue;
                }
                declare_subjects_moved.insert(m.s);
                match decl_key(snapshot, m.o, sh_prefix_id, sh_namespace_id)? {
                    Some(key) => {
                        if seen.insert(key) {
                            additions.push((root_id, declare_id, m.o));
                        }
                    }
                    // Can't determine prefix/ns: conservatively move it.
                    None => additions.push((root_id, declare_id, m.o)),
                }
            }
        }

        // Dedup additions among themselves while preserving order.
        let mut additions_set: HashSet<(u64, u64, u64)> = HashSet::new();
        additions.retain(|t| additions_set.insert(*t));

        Ok(Arc::new(Self {
            type_id,
            ontology_id,
            imports_id,
            prefixes_id,
            declare_id,
            root_id,
            imports_targets,
            remove_owl_imports,
            rewrite_sh_prefixes,
            declare_subjects_moved,
            additions,
            additions_set,
        }))
    }

    /// True if a scanned triple survives the transform's *removal* step.
    ///
    /// Does not account for `additions`; read paths union those separately.
    #[inline]
    pub fn keep(&self, s: u64, p: u64, o: u64) -> bool {
        // (1) strip resolved owl:imports.
        if self.remove_owl_imports && p == self.imports_id && self.imports_targets.contains(&o) {
            return false;
        }
        // (2) collapse non-root ontology declarations.
        if p == self.type_id && o == self.ontology_id && s != self.root_id {
            return false;
        }
        // (3) drop original SHACL prefix triples (re-added rewritten).
        if self.rewrite_sh_prefixes {
            if p == self.prefixes_id {
                return false;
            }
            if p == self.declare_id && self.declare_subjects_moved.contains(&s) {
                return false;
            }
        }
        true
    }

    /// The explicit triples this transform adds (patch graph), in term-id space.
    #[inline]
    pub fn additions(&self) -> &[(u64, u64, u64)] {
        &self.additions
    }

    /// The term id of the root ontology IRI (the flattened graph's identity).
    #[inline]
    pub fn root_id(&self) -> u64 {
        self.root_id
    }

    /// Whether `(s, p, o)` is one of the added triples.
    #[inline]
    pub fn is_addition(&self, s: u64, p: u64, o: u64) -> bool {
        self.additions_set.contains(&(s, p, o))
    }
}

/// Resolve the `(prefix, namespace)` pair of a `sh:declare` object node by
/// reading its `sh:prefix` / `sh:namespace` triples. Returns `None` when either
/// component is absent (or not a literal/IRI), signalling "can't dedup".
fn decl_key(
    snapshot: &Snapshot,
    decl_node: u64,
    sh_prefix_id: u64,
    sh_namespace_id: u64,
) -> Result<Option<(String, String)>> {
    let file = snapshot.file();
    let mut prefix: Option<String> = None;
    let mut namespace: Option<String> = None;
    let pat = Pattern {
        s: Some(decl_node),
        p: None,
        o: None,
    };
    for hit in snapshot.scan(pat, Scope::All) {
        let m = hit?;
        if m.p == sh_prefix_id {
            if let DecodedTerm::Literal { lex, .. } = file.decoded_term(m.o)? {
                prefix = Some(lex.into_owned());
            }
        } else if m.p == sh_namespace_id {
            match file.decoded_term(m.o)? {
                DecodedTerm::Iri(v) => namespace = Some(v.into_owned()),
                DecodedTerm::Literal { lex, .. } => namespace = Some(lex.into_owned()),
                DecodedTerm::BNode(_) => {}
            }
        }
    }
    Ok(match (prefix, namespace) {
        (Some(p), Some(n)) => Some((p, n)),
        _ => None,
    })
}

/// Streaming, de-duplicated iterator over the closure view's triples, in
/// term-id space, presented as a single flattened graph.
///
/// Yields `dedup(scan − removals)` followed by the `additions` that are not
/// already present in the scanned set. Callers decode the ids to terms.
pub struct ClosureTripleIds<'a> {
    snapshot: &'a Snapshot,
    patch: Arc<ClosurePatch>,
    /// Physical gids to scan (the closure's graphs, expanded to gids).
    scan: Box<dyn Iterator<Item = Result<Match>> + 'a>,
    seen: HashSet<(u64, u64, u64)>,
    /// Additions not yet emitted (drained after the scan).
    additions_idx: usize,
}

impl<'a> ClosureTripleIds<'a> {
    /// Iterate the flattened closure over the given physical gids.
    ///
    /// `scan` copies the gids internally and the returned iterator borrows only
    /// the snapshot, so `gids` need not outlive this call.
    pub fn new(snapshot: &'a Snapshot, patch: Arc<ClosurePatch>, gids: &[u64]) -> Self {
        let scan = snapshot.scan(Pattern::ANY, Scope::Gids(gids));
        Self {
            snapshot,
            patch,
            scan,
            seen: HashSet::new(),
            additions_idx: 0,
        }
    }
}

impl Iterator for ClosureTripleIds<'_> {
    type Item = Result<(u64, u64, u64)>;

    fn next(&mut self) -> Option<Self::Item> {
        // Drain the filtered, deduped scan first.
        loop {
            match self.scan.next() {
                Some(Ok(m)) => {
                    let t = (m.s, m.p, m.o);
                    if !self.patch.keep(m.s, m.p, m.o) {
                        continue;
                    }
                    if !self.seen.insert(t) {
                        continue;
                    }
                    return Some(Ok(t));
                }
                Some(Err(e)) => return Some(Err(e)),
                None => break,
            }
        }
        // Then emit additions not already produced by the scan.
        let additions = self.patch.additions();
        while self.additions_idx < additions.len() {
            let t = additions[self.additions_idx];
            self.additions_idx += 1;
            if self.seen.insert(t) {
                return Some(Ok(t));
            }
        }
        let _ = self.snapshot;
        None
    }
}

// ---------------------------------------------------------------------------
// SPARQL view
// ---------------------------------------------------------------------------

#[cfg(feature = "sparql")]
pub use sparql_view::ClosureSparqlView;

#[cfg(feature = "sparql")]
mod sparql_view {
    use super::*;
    use crate::sparql::{decoded_to_term, term_to_decoded};
    use oxrdf::Term;
    use spareval::{InternalQuad, QueryableDataset};

    /// A [`QueryableDataset`] presenting a closure as a **single flattened,
    /// de-duplicated graph**, so SPARQL over `get_closure` returns the same
    /// results as `copy_closure`.
    ///
    /// The closure's physical gids are unioned; every surviving quad is
    /// projected onto one graph name (the root), which collapses cross-graph
    /// duplicates under SPARQL's per-graph set semantics. Removals ([`keep`])
    /// and additions (the patch graph) are applied inline.
    ///
    /// [`keep`]: ClosurePatch::keep
    pub struct ClosureSparqlView<'a> {
        snapshot: &'a Snapshot,
        patch: Arc<ClosurePatch>,
        gids: Arc<Vec<u64>>,
        /// The single graph id this view exposes (the root IRI's term id).
        graph_id: u64,
    }

    impl<'a> ClosureSparqlView<'a> {
        pub fn new(snapshot: &'a Snapshot, patch: Arc<ClosurePatch>, gids: Vec<u64>) -> Self {
            let graph_id = patch.root_id;
            Self {
                snapshot,
                patch,
                gids: Arc::new(gids),
                graph_id,
            }
        }
    }

    impl<'a> QueryableDataset<'a> for ClosureSparqlView<'a> {
        type InternalTerm = u64;
        type Error = crate::reader::R5Error;

        fn internal_quads_for_pattern(
            &self,
            subject: Option<&u64>,
            predicate: Option<&u64>,
            object: Option<&u64>,
            graph_name: Option<Option<&u64>>,
        ) -> impl Iterator<Item = Result<InternalQuad<u64>>> + use<'a> {
            // Graph selection: this view has exactly one graph, `graph_id`,
            // which also answers as the default graph. `Some(Some(g))` for
            // any other id matches nothing.
            let matches_graph = match graph_name {
                None | Some(None) => true,
                Some(Some(&g)) => g == self.graph_id,
            };

            let pat = Pattern {
                s: subject.copied(),
                p: predicate.copied(),
                o: object.copied(),
            };
            let patch = self.patch.clone();
            let graph_id = self.graph_id;
            let gids = self.gids.clone();
            let snapshot = self.snapshot;

            let empty = !matches_graph;
            // Surviving, de-duplicated scanned quads projected onto `graph_id`.
            let mut seen: HashSet<(u64, u64, u64)> = HashSet::new();
            let scan_patch = patch.clone();
            let scanned = snapshot
                .scan(pat, Scope::Gids(&gids))
                .filter_map(move |hit| match hit {
                    Ok(m) => {
                        if !scan_patch.keep(m.s, m.p, m.o) {
                            return None;
                        }
                        if !seen.insert((m.s, m.p, m.o)) {
                            return None;
                        }
                        Some(Ok(InternalQuad {
                            subject: m.s,
                            predicate: m.p,
                            object: m.o,
                            graph_name: Some(graph_id),
                        }))
                    }
                    Err(e) => Some(Err(e)),
                });

            // Additions matching the pattern (the scan can't produce them).
            let add_pat = pat;
            let additions: Vec<Result<InternalQuad<u64>>> = patch
                .additions()
                .iter()
                .filter(move |(s, p, o)| {
                    add_pat.s.is_none_or(|x| x == *s)
                        && add_pat.p.is_none_or(|x| x == *p)
                        && add_pat.o.is_none_or(|x| x == *o)
                })
                .map(move |&(s, p, o)| {
                    Ok(InternalQuad {
                        subject: s,
                        predicate: p,
                        object: o,
                        graph_name: Some(graph_id),
                    })
                })
                .collect();

            let iter: Box<dyn Iterator<Item = Result<InternalQuad<u64>>>> = if empty {
                Box::new(std::iter::empty())
            } else {
                Box::new(scanned.chain(additions))
            };
            iter
        }

        fn internal_named_graphs(&self) -> impl Iterator<Item = Result<u64>> + use<'a> {
            std::iter::once(Ok(self.graph_id))
        }

        fn contains_internal_graph_name(&self, graph_name: &u64) -> Result<bool> {
            Ok(*graph_name == self.graph_id)
        }

        fn internalize_term(&self, term: Term) -> Result<u64> {
            Ok(self.snapshot.file().intern_decoded(&term_to_decoded(&term)))
        }

        fn externalize_term(&self, term: u64) -> Result<Term> {
            decoded_to_term(self.snapshot.file().externalize_id(term)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Snapshot;
    use crate::{Quint, Term, write_file};
    use std::collections::HashSet;
    use std::path::Path;
    use tempfile::tempdir;

    const TYPE: &str = RDF_TYPE;
    const ONT: &str = OWL_ONTOLOGY;
    const IMPORTS: &str = OWL_IMPORTS;
    const PREFIXES: &str = SH_PREFIXES;
    const DECLARE: &str = SH_DECLARE;
    const PREFIX: &str = SH_PREFIX;
    const NS: &str = SH_NAMESPACE;

    fn iri(s: &str) -> Term {
        Term::Iri(s.into())
    }
    fn lit(s: &str) -> Term {
        Term::Literal {
            lex: s.into(),
            dt: None,
            lang: None,
        }
    }

    fn snap(path: &Path, quints: &[Quint]) -> Snapshot {
        write_file(path, quints).expect("write fixture");
        Snapshot::open(path).expect("open snapshot")
    }

    /// Decode the flattened closure into a set of `(s, p, o)` string triples.
    fn closure_set(
        snapshot: &Snapshot,
        patch: Arc<ClosurePatch>,
        gids: &[u64],
    ) -> HashSet<(String, String, String)> {
        let file = snapshot.file();
        let dec = |id: u64| match file.externalize_id(id).unwrap() {
            DecodedTerm::Iri(v) => v.into_owned(),
            DecodedTerm::BNode(v) => format!("_:{}", v),
            DecodedTerm::Literal { lex, .. } => format!("\"{}\"", lex),
        };
        ClosureTripleIds::new(snapshot, patch, gids)
            .map(|r| {
                let (s, p, o) = r.unwrap();
                (dec(s), dec(p), dec(o))
            })
            .collect()
    }

    fn all_gids(snapshot: &Snapshot) -> Vec<u64> {
        let mut gids = Vec::new();
        for name in snapshot
            .graph_names()
            .map(str::to_string)
            .collect::<Vec<_>>()
        {
            if let Some(g) = snapshot.gids_for_name(&name) {
                gids.extend_from_slice(g);
            }
        }
        gids
    }

    #[test]
    fn strips_imports_collapses_decls_and_adds_root() {
        // root imports mid; mid imports leaf. Root does NOT declare itself.
        let root = "http://ex/root";
        let mid = "http://ex/mid";
        let leaf = "http://ex/leaf";
        let dir = tempdir().unwrap();
        let path = dir.path().join("c.r5tu");
        let quints = vec![
            // root graph: no self-declaration, imports mid
            Quint {
                id: root.into(),
                s: iri(root),
                p: iri(IMPORTS),
                o: iri(mid),
                gname: root.into(),
            },
            Quint {
                id: root.into(),
                s: iri("http://ex/RootClass"),
                p: iri(TYPE),
                o: iri(ONT.replace("Ontology", "Class").as_str()),
                gname: root.into(),
            },
            // mid graph: declares itself, imports leaf
            Quint {
                id: mid.into(),
                s: iri(mid),
                p: iri(TYPE),
                o: iri(ONT),
                gname: mid.into(),
            },
            Quint {
                id: mid.into(),
                s: iri(mid),
                p: iri(IMPORTS),
                o: iri(leaf),
                gname: mid.into(),
            },
            Quint {
                id: mid.into(),
                s: iri("http://ex/Shared"),
                p: iri(TYPE),
                o: iri("http://ex/C"),
                gname: mid.into(),
            },
            // leaf graph: declares itself, has the SAME shared triple (cross-graph dup)
            Quint {
                id: leaf.into(),
                s: iri(leaf),
                p: iri(TYPE),
                o: iri(ONT),
                gname: leaf.into(),
            },
            Quint {
                id: leaf.into(),
                s: iri("http://ex/Shared"),
                p: iri(TYPE),
                o: iri("http://ex/C"),
                gname: leaf.into(),
            },
        ];
        let s = snap(&path, &quints);
        let gids = all_gids(&s);
        let closure_iris = vec![root.to_string(), mid.to_string(), leaf.to_string()];
        let patch = ClosurePatch::build(&s, root, &closure_iris, true, true).unwrap();
        let set = closure_set(&s, patch, &gids);

        // resolved owl:imports stripped
        assert!(
            !set.iter().any(|(_, p, _)| p == IMPORTS),
            "imports leaked: {set:?}"
        );
        // only the root ontology declaration remains
        let decls: Vec<_> = set
            .iter()
            .filter(|(_, p, o)| p == TYPE && o == ONT)
            .collect();
        assert_eq!(decls.len(), 1, "expected 1 ontology decl, got {decls:?}");
        assert_eq!(decls[0].0, root);
        // additive root declaration is present even though root didn't declare itself
        assert!(set.contains(&(root.to_string(), TYPE.to_string(), ONT.to_string())));
        // cross-graph duplicate collapsed to one
        let shared: Vec<_> = set
            .iter()
            .filter(|(sub, _, _)| sub == "http://ex/Shared")
            .collect();
        assert_eq!(shared.len(), 1, "shared triple not deduped: {shared:?}");
    }

    #[test]
    fn rewrites_sh_prefixes_onto_root() {
        let root = "http://ex/root";
        let leaf = "http://ex/leaf";
        let dir = tempdir().unwrap();
        let path = dir.path().join("p.r5tu");
        let quints = vec![
            Quint {
                id: root.into(),
                s: iri(root),
                p: iri(TYPE),
                o: iri(ONT),
                gname: root.into(),
            },
            Quint {
                id: root.into(),
                s: iri(root),
                p: iri(IMPORTS),
                o: iri(leaf),
                gname: root.into(),
            },
            // leaf: sh:prefixes leaf ; sh:declare _:d ; _:d sh:prefix "leaf"; sh:namespace "..."
            Quint {
                id: leaf.into(),
                s: iri(leaf),
                p: iri(TYPE),
                o: iri(ONT),
                gname: leaf.into(),
            },
            Quint {
                id: leaf.into(),
                s: iri(leaf),
                p: iri(PREFIXES),
                o: iri(leaf),
                gname: leaf.into(),
            },
            Quint {
                id: leaf.into(),
                s: iri(leaf),
                p: iri(DECLARE),
                o: Term::BNode("_:d".into()),
                gname: leaf.into(),
            },
            Quint {
                id: leaf.into(),
                s: Term::BNode("_:d".into()),
                p: iri(PREFIX),
                o: lit("leaf"),
                gname: leaf.into(),
            },
            Quint {
                id: leaf.into(),
                s: Term::BNode("_:d".into()),
                p: iri(NS),
                o: lit("http://ex/leaf#"),
                gname: leaf.into(),
            },
        ];
        let s = snap(&path, &quints);
        let gids = all_gids(&s);
        let closure_iris = vec![root.to_string(), leaf.to_string()];
        let patch = ClosurePatch::build(&s, root, &closure_iris, true, true).unwrap();
        let set = closure_set(&s, patch, &gids);

        // sh:prefixes now points at the root, and none point at leaf.
        assert!(set.contains(&(leaf.to_string(), PREFIXES.to_string(), root.to_string())));
        assert!(!set.contains(&(leaf.to_string(), PREFIXES.to_string(), leaf.to_string())));
        // sh:declare relocated onto the root.
        assert!(set.iter().any(|(sub, p, _)| sub == root && p == DECLARE));
        assert!(!set.iter().any(|(sub, p, _)| sub == leaf && p == DECLARE));
    }

    #[test]
    fn no_rewrite_when_disabled_keeps_imports_and_prefixes() {
        let root = "http://ex/root";
        let leaf = "http://ex/leaf";
        let dir = tempdir().unwrap();
        let path = dir.path().join("raw.r5tu");
        let quints = vec![
            Quint {
                id: root.into(),
                s: iri(root),
                p: iri(TYPE),
                o: iri(ONT),
                gname: root.into(),
            },
            Quint {
                id: root.into(),
                s: iri(root),
                p: iri(IMPORTS),
                o: iri(leaf),
                gname: root.into(),
            },
            Quint {
                id: leaf.into(),
                s: iri(leaf),
                p: iri(TYPE),
                o: iri(ONT),
                gname: leaf.into(),
            },
            Quint {
                id: leaf.into(),
                s: iri(leaf),
                p: iri(PREFIXES),
                o: iri(leaf),
                gname: leaf.into(),
            },
        ];
        let s = snap(&path, &quints);
        let gids = all_gids(&s);
        let closure_iris = vec![root.to_string(), leaf.to_string()];
        // remove_owl_imports = false, rewrite_sh_prefixes = false
        let patch = ClosurePatch::build(&s, root, &closure_iris, false, false).unwrap();
        let set = closure_set(&s, patch, &gids);

        // imports kept
        assert!(set.contains(&(root.to_string(), IMPORTS.to_string(), leaf.to_string())));
        // original sh:prefixes kept (not rewritten)
        assert!(set.contains(&(leaf.to_string(), PREFIXES.to_string(), leaf.to_string())));
        // BUT non-root ontology decls are always collapsed
        let decls: Vec<_> = set
            .iter()
            .filter(|(_, p, o)| p == TYPE && o == ONT)
            .collect();
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].0, root);
    }
}
