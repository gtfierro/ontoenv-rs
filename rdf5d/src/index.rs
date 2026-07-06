//! In-memory permutation indexes for R5TU files.
//!
//! These indexes are built on demand from an open [`R5tuFile`] and held in RAM
//! as plain Rust collections — there is no on-disk sidecar and no byte-packed
//! representation. A snapshot is immutable for its lifetime, so each index is
//! built once and never needs validation against disk.
//!
//! - [`MemSection`] holds one permutation. It maps a key term id to a
//!   [`Posting`]: the gids that contain the key, each with its `(A, B)` pairs.
//!   The permutations are:
//!     * PSO (`predicate → subject → object`) / POS (`predicate → object →
//!       subject`) for a bound predicate;
//!     * SPO (`subject → predicate → object`) for a bound subject;
//!     * OSP (`object → subject → predicate`) for a bound object.
//! - [`MemPClos`] precomputes the transitive closure (both directions) of a
//!   configured set of predicates, for SPARQL `P+`/`P*` property paths.
//!
//! Build with [`build_mem_section`] / [`build_mem_pclos`]; read with
//! [`MemSection::lookup`] / [`MemPClos::closure_forward`].

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use crate::reader::{R5Error, R5tuFile, Result};

/// The permutation a [`MemSection`] indexes (which term is the key, and the
/// order of the inner pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdxKind {
    /// `predicate → subject → object`. Serves patterns with a bound predicate.
    Pso,
    /// `predicate → object → subject`. Serves patterns with a bound predicate
    /// and object.
    Pos,
    /// Precomputed transitive-closure index for a configured list of
    /// predicates (`P+`/`P*` SPARQL property paths). Built via
    /// [`build_mem_pclos`], not [`build_mem_section`].
    PClos,
    /// `subject → predicate → object`. Serves patterns with a bound subject
    /// and an unbound predicate (`(s, ?, ?)` / `(s, ?, o)`).
    Spo,
    /// `object → subject → predicate`. Serves patterns with a bound object and
    /// an unbound predicate (`(?, ?, o)`).
    Osp,
}

/// One key's postings: the gids containing the key, each with its `(A, B)`
/// pairs. `gids` is sorted ascending and parallel to `blocks`. The meaning of
/// `A`/`B` depends on the permutation (see [`build_mem_section`]).
#[derive(Debug, Default)]
pub struct Posting {
    pub(crate) gids: Vec<u32>,
    pub(crate) blocks: Vec<Vec<(u64, u64)>>,
}

/// A borrowed view of one key's [`Posting`].
#[derive(Debug, Clone, Copy)]
pub struct IdxPosting<'a> {
    posting: &'a Posting,
}

impl<'a> IdxPosting<'a> {
    pub fn n_gids(&self) -> u32 {
        self.posting.gids.len() as u32
    }

    pub fn n_triples(&self) -> u32 {
        self.posting.blocks.iter().map(|b| b.len() as u32).sum()
    }

    pub fn gids(&self) -> Vec<u64> {
        self.posting.gids.iter().map(|&g| g as u64).collect()
    }

    /// Binary search for a gid, returning its block index.
    pub fn block_for_gid(&self, gid: u64) -> Option<usize> {
        if gid > u32::MAX as u64 {
            return None;
        }
        self.posting.gids.binary_search(&(gid as u32)).ok()
    }

    /// Iterate the `(A, B)` pairs of one block (one gid).
    ///
    /// Takes `self` by value (the view is `Copy`) so the returned iterator
    /// borrows only the underlying posting (`'a`), not the local view — letting
    /// it stream out of the closures in [`crate::snapshot::Snapshot::scan`].
    pub fn iter_block(self, block_idx: usize) -> impl Iterator<Item = (u64, u64)> + 'a {
        self.posting.blocks[block_idx].iter().copied()
    }

    /// Iterate `(gid, A, B)` across every block.
    pub fn iter_all(self) -> impl Iterator<Item = (u64, u64, u64)> + 'a {
        let posting = self.posting;
        posting
            .gids
            .iter()
            .zip(posting.blocks.iter())
            .flat_map(|(&gid, block)| block.iter().map(move |&(a, b)| (gid as u64, a, b)))
    }
}

/// One permutation index, built in memory from an [`R5tuFile`].
///
/// Keys are held sorted in `keys`, parallel to `postings`, so a lookup is a
/// binary search — no hashing, and cache-friendly to build (the keys come out
/// of the sorted tuple stream already in order).
#[derive(Debug)]
pub struct MemSection {
    pub(crate) kind: IdxKind,
    pub(crate) keys: Vec<u64>,
    pub(crate) postings: Vec<Posting>,
}

impl MemSection {
    /// The permutation this section indexes.
    pub fn kind(&self) -> IdxKind {
        self.kind
    }

    /// Look up the posting for `key` (a predicate id for PSO/POS, subject id
    /// for SPO, object id for OSP). The posting's `(A, B)` pairs follow the
    /// permutation's convention — see [`build_mem_section`].
    pub fn lookup(&self, key: u64) -> Option<IdxPosting<'_>> {
        let idx = self.keys.binary_search(&key).ok()?;
        Some(IdxPosting {
            posting: &self.postings[idx],
        })
    }
}

/// Walk every triple in the snapshot into a `(p, gid, s, o)` tuple vector.
fn collect_tuples(r5tu: &R5tuFile) -> Result<Vec<(u64, u32, u64, u64)>> {
    let graphs = r5tu.enumerate_all()?;
    let total: u64 = graphs.iter().map(|g| g.n_triples).sum();
    let mut tuples: Vec<(u64, u32, u64, u64)> = Vec::with_capacity(total as usize);
    for g in &graphs {
        if g.gid > u32::MAX as u64 {
            return Err(R5Error::Invalid("gid exceeds u32 (index limit)"));
        }
        for (s, p, o) in r5tu.triples_ids(g.gid)? {
            tuples.push((p, g.gid as u32, s, o));
        }
    }
    Ok(tuples)
}

/// Build one permutation index in memory from an open snapshot. Accepts `Pso`,
/// `Pos`, `Spo`, or `Osp`; use [`build_mem_pclos`] for `PClos`.
///
/// The key dimension and the meaning of `A`/`B`:
///   PSO: key = p, A = s, B = o      POS: key = p, A = o, B = s
///   SPO: key = s, A = p, B = o      OSP: key = o, A = s, B = p
pub fn build_mem_section(r5tu: &R5tuFile, kind: IdxKind) -> Result<MemSection> {
    if kind == IdxKind::PClos {
        return Err(R5Error::Invalid("use build_mem_pclos for PClos"));
    }

    // Remap each triple to (key, gid, A, B) for this permutation, then sort
    // once. Sorting groups equal keys and gids into contiguous runs and leaves
    // the (A, B) pairs ordered, so the postings build in a single linear walk
    // with no per-triple hashing.
    let mut tuples: Vec<(u64, u32, u64, u64)> = collect_tuples(r5tu)?
        .into_iter()
        .map(|(p, gid, s, o)| match kind {
            IdxKind::Pso => (p, gid, s, o),
            IdxKind::Pos => (p, gid, o, s),
            IdxKind::Spo => (s, gid, p, o),
            IdxKind::Osp => (o, gid, s, p),
            IdxKind::PClos => unreachable!(),
        })
        .collect();
    tuples.sort_unstable();

    let mut keys: Vec<u64> = Vec::new();
    let mut postings: Vec<Posting> = Vec::new();
    let mut i = 0usize;
    while i < tuples.len() {
        let key = tuples[i].0;
        let mut gids: Vec<u32> = Vec::new();
        let mut blocks: Vec<Vec<(u64, u64)>> = Vec::new();
        while i < tuples.len() && tuples[i].0 == key {
            let gid = tuples[i].1;
            let mut block: Vec<(u64, u64)> = Vec::new();
            while i < tuples.len() && tuples[i].0 == key && tuples[i].1 == gid {
                block.push((tuples[i].2, tuples[i].3));
                i += 1;
            }
            gids.push(gid);
            blocks.push(block);
        }
        keys.push(key);
        postings.push(Posting { gids, blocks });
    }

    Ok(MemSection { kind, keys, postings })
}

// ============================================================================
// PClos: precomputed transitive-closure index
// ============================================================================

/// Both directions of the transitive closure for one predicate.
#[derive(Debug, Default)]
pub(crate) struct PClosPred {
    /// subject -> sorted set of reachable objects.
    pub(crate) forward: HashMap<u64, Vec<u64>>,
    /// object -> sorted set of subjects that reach it transitively.
    pub(crate) reverse: HashMap<u64, Vec<u64>>,
}

/// An in-memory precomputed transitive-closure index over a configured set of
/// predicates, built directly from an [`R5tuFile`].
#[derive(Debug)]
pub struct MemPClos {
    pub(crate) preds: HashMap<u64, PClosPred>,
}

impl MemPClos {
    /// Forward closure: objects reachable from `subject` via one or more
    /// `predicate` edges, sorted; `None` if `predicate` has no precomputed
    /// closure or `subject` has no outgoing edges under it.
    pub fn closure_forward(&self, predicate: u64, subject: u64) -> Option<Vec<u64>> {
        self.preds.get(&predicate)?.forward.get(&subject).cloned()
    }

    /// Reverse closure: subjects that reach `object` via one or more
    /// `predicate` edges, sorted.
    pub fn closure_reverse(&self, predicate: u64, object: u64) -> Option<Vec<u64>> {
        self.preds.get(&predicate)?.reverse.get(&object).cloned()
    }
}

/// Build the in-memory transitive-closure index for `predicates`.
///
/// The closure is whole-snapshot (gid is ignored) and non-reflexive; callers
/// that want `P*` semantics add the source node themselves. Cycles are handled
/// by the BFS visited set.
pub fn build_mem_pclos(r5tu: &R5tuFile, predicates: &[u64]) -> Result<MemPClos> {
    let tuples = collect_tuples(r5tu)?;
    let mut wanted: Vec<u64> = predicates.to_vec();
    wanted.sort_unstable();
    wanted.dedup();

    let mut preds = HashMap::with_capacity(wanted.len());
    for p in wanted {
        // Distinct (s, o) edges for predicate p, ignoring gid.
        let mut adjacency: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
        for &(tp, _gid, s, o) in &tuples {
            if tp == p {
                adjacency.entry(s).or_default().insert(o);
            }
        }
        if adjacency.is_empty() {
            preds.insert(p, PClosPred::default());
            continue;
        }
        let forward = bfs_closure_table(&adjacency);
        // Invert and BFS for the reverse direction.
        let mut inverse: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
        for (s, objects) in &adjacency {
            for o in objects {
                inverse.entry(*o).or_default().insert(*s);
            }
        }
        let reverse = bfs_closure_table(&inverse);
        preds.insert(
            p,
            PClosPred {
                forward: forward.into_iter().collect(),
                reverse: reverse.into_iter().collect(),
            },
        );
    }

    Ok(MemPClos { preds })
}

/// For every source node in `adj`, compute the set of nodes reachable via one
/// or more steps (non-reflexive). Iterative BFS with a visited set; safe
/// against cycles. Returned reachable sets are sorted ascending.
fn bfs_closure_table(adj: &BTreeMap<u64, BTreeSet<u64>>) -> BTreeMap<u64, Vec<u64>> {
    let mut out = BTreeMap::new();
    for start in adj.keys() {
        let mut visited = BTreeSet::new();
        let mut queue: VecDeque<u64> = VecDeque::new();
        if let Some(direct) = adj.get(start) {
            for d in direct {
                if visited.insert(*d) {
                    queue.push_back(*d);
                }
            }
        }
        while let Some(cur) = queue.pop_front() {
            if let Some(next) = adj.get(&cur) {
                for n in next {
                    if visited.insert(*n) {
                        queue.push_back(*n);
                    }
                }
            }
        }
        if !visited.is_empty() {
            out.insert(*start, visited.into_iter().collect());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Quint, Term, write_file};
    use tempfile::tempdir;

    fn iri(s: &str) -> Term {
        Term::Iri(s.into())
    }

    fn term_id(f: &R5tuFile, s: &str) -> u64 {
        f.find_decoded_term(&crate::DecodedTerm::Iri(std::borrow::Cow::Borrowed(s)))
            .unwrap()
            .unwrap_or_else(|| panic!("missing term {s}"))
    }

    /// Three gids; s1 has (p1,o1) and (p1,o3), s2 has (p2,o2), in each gid.
    fn sample_file(dir: &std::path::Path) -> R5tuFile {
        let r5tu = dir.join("store.r5tu");
        let mut quads = Vec::new();
        for (id, gname) in [("d1", "g1"), ("d2", "g2"), ("d3", "g3")] {
            quads.push(Quint { id: id.into(), s: iri("http://ex/s1"), p: iri("http://ex/p1"), o: iri("http://ex/o1"), gname: gname.into() });
            quads.push(Quint { id: id.into(), s: iri("http://ex/s2"), p: iri("http://ex/p2"), o: iri("http://ex/o2"), gname: gname.into() });
            quads.push(Quint { id: id.into(), s: iri("http://ex/s1"), p: iri("http://ex/p1"), o: iri("http://ex/o3"), gname: gname.into() });
        }
        write_file(&r5tu, &quads).unwrap();
        R5tuFile::open(&r5tu).unwrap()
    }

    #[test]
    fn pso_pos_mem_lookup() {
        let dir = tempdir().unwrap();
        let f = sample_file(dir.path());
        let pso = build_mem_section(&f, IdxKind::Pso).unwrap();
        let pos = build_mem_section(&f, IdxKind::Pos).unwrap();
        let p1 = term_id(&f, "http://ex/p1");

        let post = pso.lookup(p1).expect("p1 pso posting");
        assert_eq!(post.n_gids(), 3);
        assert_eq!(post.iter_all().count(), 6); // (s1,o1) and (s1,o3) x 3 gids
        assert_eq!(pos.lookup(p1).expect("p1 pos posting").iter_all().count(), 6);
        assert!(pso.lookup(999_999).is_none());
    }

    #[test]
    fn spo_osp_mem_lookup() {
        let dir = tempdir().unwrap();
        let f = sample_file(dir.path());
        let spo = build_mem_section(&f, IdxKind::Spo).unwrap();
        let osp = build_mem_section(&f, IdxKind::Osp).unwrap();
        let (s1, p1, o1, o3) = (
            term_id(&f, "http://ex/s1"),
            term_id(&f, "http://ex/p1"),
            term_id(&f, "http://ex/o1"),
            term_id(&f, "http://ex/o3"),
        );

        // SPO: subject s1 -> (predicate, object) pairs. 2 per gid x 3.
        let post = spo.lookup(s1).expect("s1 spo posting");
        assert_eq!(post.n_gids(), 3);
        let pairs: Vec<(u64, u64)> = post.iter_all().map(|(_, p, o)| (p, o)).collect();
        assert_eq!(pairs.len(), 6);
        assert!(pairs.iter().all(|&(p, _)| p == p1));
        assert!(pairs.iter().any(|&(_, o)| o == o1));
        assert!(pairs.iter().any(|&(_, o)| o == o3));

        // OSP: object o1 -> (subject, predicate) pairs. once per gid x 3.
        let post = osp.lookup(o1).expect("o1 osp posting");
        assert_eq!(post.n_gids(), 3);
        let pairs: Vec<(u64, u64)> = post.iter_all().map(|(_, s, p)| (s, p)).collect();
        assert_eq!(pairs.len(), 3);
        assert!(pairs.iter().all(|&(s, p)| s == s1 && p == p1));

        assert!(spo.lookup(999_999).is_none());
    }

    #[test]
    fn pso_pos_values_match() {
        let dir = tempdir().unwrap();
        let r5tu = dir.path().join("store.r5tu");
        let quads = vec![
            Quint { id: "d1".into(), s: iri("http://ex/s1"), p: iri("http://ex/p1"), o: iri("http://ex/o1"), gname: "g1".into() },
            Quint { id: "d1".into(), s: iri("http://ex/s2"), p: iri("http://ex/p1"), o: iri("http://ex/o2"), gname: "g1".into() },
            Quint { id: "d2".into(), s: iri("http://ex/s3"), p: iri("http://ex/p1"), o: iri("http://ex/o1"), gname: "g2".into() },
        ];
        write_file(&r5tu, &quads).unwrap();
        let f = R5tuFile::open(&r5tu).unwrap();
        let pso = build_mem_section(&f, IdxKind::Pso).unwrap();
        let pos = build_mem_section(&f, IdxKind::Pos).unwrap();
        let p1 = term_id(&f, "http://ex/p1");

        let mut expected: Vec<(u64, u64, u64)> = Vec::new();
        for gr in f.enumerate_all().unwrap() {
            for (s, p, o) in f.triples_ids(gr.gid).unwrap() {
                if p == p1 {
                    expected.push((gr.gid, s, o));
                }
            }
        }
        expected.sort();

        let mut got: Vec<(u64, u64, u64)> = pso.lookup(p1).unwrap().iter_all().collect();
        got.sort();
        assert_eq!(expected, got);

        // POS iter_all yields (gid, o, s); swap back to (gid, s, o).
        let mut got_pos: Vec<(u64, u64, u64)> =
            pos.lookup(p1).unwrap().iter_all().map(|(gid, o, s)| (gid, s, o)).collect();
        got_pos.sort();
        assert_eq!(expected, got_pos);
    }

    #[test]
    fn block_for_gid_lookup() {
        let dir = tempdir().unwrap();
        let r5tu = dir.path().join("store.r5tu");
        let quads = vec![
            Quint { id: "d1".into(), s: iri("http://ex/s1"), p: iri("http://ex/p1"), o: iri("http://ex/o1"), gname: "g1".into() },
            Quint { id: "d2".into(), s: iri("http://ex/s2"), p: iri("http://ex/p1"), o: iri("http://ex/o2"), gname: "g2".into() },
        ];
        write_file(&r5tu, &quads).unwrap();
        let f = R5tuFile::open(&r5tu).unwrap();
        let pso = build_mem_section(&f, IdxKind::Pso).unwrap();
        let post = pso.lookup(term_id(&f, "http://ex/p1")).unwrap();
        let gids = post.gids();
        for (idx, g) in gids.iter().enumerate() {
            assert_eq!(post.block_for_gid(*g), Some(idx));
        }
        assert_eq!(post.block_for_gid(99), None);
    }

    #[test]
    fn pclos_roundtrip_and_lookup() {
        // Class hierarchy: A <- B <- C, B <- D, E <- F  (X <- Y: Y subClassOf X)
        let dir = tempdir().unwrap();
        let r5tu = dir.path().join("store.r5tu");
        let subclass = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
        let edge = |s: &str, o: &str| Quint {
            id: "d1".into(), s: iri(s), p: iri(subclass), o: iri(o), gname: "g1".into(),
        };
        let quads = vec![
            edge("http://ex/B", "http://ex/A"),
            edge("http://ex/C", "http://ex/B"),
            edge("http://ex/D", "http://ex/B"),
            edge("http://ex/F", "http://ex/E"),
        ];
        write_file(&r5tu, &quads).unwrap();
        let f = R5tuFile::open(&r5tu).unwrap();
        let p_id = term_id(&f, subclass);
        let pclos = build_mem_pclos(&f, &[p_id]).unwrap();

        let id = |s: &str| term_id(&f, s);
        let (id_a, id_b, id_c, id_d, id_e, id_f) = (
            id("http://ex/A"), id("http://ex/B"), id("http://ex/C"),
            id("http://ex/D"), id("http://ex/E"), id("http://ex/F"),
        );

        // Forward (subject -> reachable objects).
        let mut got = pclos.closure_forward(p_id, id_c).unwrap();
        got.sort();
        let mut want = vec![id_a, id_b];
        want.sort();
        assert_eq!(got, want);
        assert_eq!(pclos.closure_forward(p_id, id_b).unwrap(), vec![id_a]);
        assert_eq!(pclos.closure_forward(p_id, id_f).unwrap(), vec![id_e]);
        assert!(pclos.closure_forward(p_id, id_a).is_none()); // leaf target

        // Reverse (object -> reachable subjects).
        let mut got = pclos.closure_reverse(p_id, id_a).unwrap();
        got.sort();
        let mut want = vec![id_b, id_c, id_d];
        want.sort();
        assert_eq!(got, want);
        assert_eq!(pclos.closure_reverse(p_id, id_e).unwrap(), vec![id_f]);

        // Predicate not in build list: no closure data.
        assert!(pclos.closure_forward(999_999, id_a).is_none());
    }

    #[test]
    fn pclos_cycle_handling() {
        // A -> B -> A. Non-reflexive closure: A reaches {B}, B reaches {A}.
        let dir = tempdir().unwrap();
        let r5tu = dir.path().join("store.r5tu");
        let p = "http://ex/p";
        let quads = vec![
            Quint { id: "d1".into(), s: iri("http://ex/A"), p: iri(p), o: iri("http://ex/B"), gname: "g1".into() },
            Quint { id: "d1".into(), s: iri("http://ex/B"), p: iri(p), o: iri("http://ex/A"), gname: "g1".into() },
        ];
        write_file(&r5tu, &quads).unwrap();
        let f = R5tuFile::open(&r5tu).unwrap();
        let p_id = term_id(&f, p);
        let a = term_id(&f, "http://ex/A");
        let b = term_id(&f, "http://ex/B");
        let pclos = build_mem_pclos(&f, &[p_id]).unwrap();
        assert!(pclos.closure_forward(p_id, a).unwrap().contains(&b));
        assert!(pclos.closure_forward(p_id, b).unwrap().contains(&a));
    }
}
