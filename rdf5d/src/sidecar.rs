//! In-memory permutation indexes for R5TU files.
//!
//! These indexes are built on demand from an open [`R5tuFile`] and held in
//! RAM — there is no on-disk sidecar. Each [`MemSection`] holds one
//! permutation's posting lists in a compact, binary-searchable layout:
//!
//! - PSO (`predicate → subject → object`) and POS (`predicate → object →
//!   subject`) for patterns with a bound predicate;
//! - SPO (`subject → predicate → object`) for a bound subject;
//! - OSP (`object → subject → predicate`) for a bound object.
//!
//! [`MemPClos`] additionally precomputes the transitive closure of a
//! configured set of predicates (for SPARQL `P+`/`P*` property paths).
//!
//! Indexes carry no data that isn't derivable from the source `.r5tu` file,
//! so they are always fresh by construction (built from the snapshot you just
//! opened) and need no validation against disk.
//!
//! Each section stores:
//!
//! - `pred_keys[n_predicates]` — sorted u64 key term IDs (binary-searchable).
//! - `key2post_offs[n_predicates + 1]` — slice table into the per-key blob.
//! - Concatenated per-key postings, each laid out as:
//!
//!   * `gid_vals[n_gids]`, `s_heads[n_gids + 1]`, `s_byte_heads[n_gids + 1]`
//!     (fixed-width u32, for O(1) random access into one gid's run).
//!   * Uvarint+delta `s_vals` and `o_vals` streams (compact bulk payload).
//!
//! Use [`build_mem_section`] / [`build_mem_pclos`] to build, and
//! [`MemSection::lookup`] / [`MemPClos::closure_forward`] to read.

use crate::reader::{R5Error, R5tuFile, Result};

/// Sidecar magic at the start of every sidecar file.
pub const SIDECAR_MAGIC: &[u8; 5] = b"R5IDX";

/// Sidecar end-of-file marker (12 bytes).
pub const SIDECAR_EOF_MAGIC: &[u8; 12] = b"R5IDX_ENDM\0\0";

/// Current sidecar format version.
pub const SIDECAR_VERSION: u16 = 0x0001;

/// Kinds of sections within a sidecar file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum IdxKind {
    Pso = 1,
    Pos = 2,
    /// Precomputed transitive-closure index for a configured list of
    /// predicates (`P+` for SPARQL property paths). For each predicate `P`
    /// in the list, stores both directions of the transitive closure:
    /// forward (subject -> reachable objects) and reverse (object -> reachable
    /// subjects). Used by the SPARQL query rewriter to replace
    /// `?x P+ ?y` / `?x P* ?y` with materialized `VALUES`/`BIND` bindings
    /// before passing the query to spareval.
    PClos = 3,
    /// Subject-keyed posting list (`subject → predicate → object`). Serves
    /// triple patterns with a bound subject and an unbound predicate
    /// (`(s, ?, ?)` / `(s, ?, o)`) without scanning every graph.
    Spo = 4,
    /// Object-keyed posting list (`object → subject → predicate`). Serves
    /// triple patterns with a bound object and an unbound predicate
    /// (`(?, ?, o)`) without scanning every graph.
    Osp = 5,
}

impl IdxKind {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            1 => Some(IdxKind::Pso),
            2 => Some(IdxKind::Pos),
            3 => Some(IdxKind::PClos),
            4 => Some(IdxKind::Spo),
            5 => Some(IdxKind::Osp),
            _ => None,
        }
    }
}

/// Read-only view of one predicate's PSO/POS posting block.
///
/// In PSO, the inner dimension is S; in POS, it is O. The shape is identical
/// — only the semantics of the inner stream differ. Field names follow the
/// PSO convention (`s_*`, `o_*`) for clarity; for POS, "s" means "object" and
/// "o" means "subject".
#[derive(Debug, Clone)]
pub struct IdxPosting<'a> {
    bytes: &'a [u8],
    n_gids: u32,
    gid_vals_off: usize,
    s_heads_off: usize,
    s_byte_heads_off: usize,
    #[allow(dead_code)]
    n_s: u32,
    s_vals_off: usize,
    o_heads_off: usize,
    o_byte_heads_off: usize,
    n_t: u32,
    o_vals_off: usize,
}

impl<'a> IdxPosting<'a> {
    pub fn n_gids(&self) -> u32 {
        self.n_gids
    }
    pub fn n_triples(&self) -> u32 {
        self.n_t
    }
    pub fn gids(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.n_gids as usize);
        for i in 0..self.n_gids as usize {
            let o = self.gid_vals_off + i * 4;
            out.push(u32::from_le_bytes(self.bytes[o..o + 4].try_into().unwrap()) as u64);
        }
        out
    }
    /// Binary search for a gid in `gid_vals`, returning the block index.
    pub fn block_for_gid(&self, gid: u64) -> Option<usize> {
        if gid > u32::MAX as u64 {
            return None;
        }
        let g = gid as u32;
        let n = self.n_gids as usize;
        let mut lo = 0usize;
        let mut hi = n;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let off = self.gid_vals_off + mid * 4;
            let v = u32::from_le_bytes(self.bytes[off..off + 4].try_into().unwrap());
            if v == g {
                return Some(mid);
            } else if v < g {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        None
    }
    fn u32_at(&self, base: usize, idx: usize) -> u32 {
        let o = base + idx * 4;
        u32::from_le_bytes(self.bytes[o..o + 4].try_into().unwrap())
    }
    /// Iterate `(S, O)` pairs (or `(O, S)` for POS, depending on permutation)
    /// for a given block index.
    pub fn iter_block(&self, block_idx: usize) -> IdxBlockIter<'a> {
        let s_start = self.u32_at(self.s_heads_off, block_idx) as usize;
        let s_end = self.u32_at(self.s_heads_off, block_idx + 1) as usize;
        let s_byte = self.u32_at(self.s_byte_heads_off, block_idx) as usize;
        IdxBlockIter {
            bytes: self.bytes,
            s_idx: s_start,
            s_end,
            s_byte_off: self.s_vals_off + s_byte,
            current_s: 0,
            need_s: true,
            o_heads_off: self.o_heads_off,
            o_byte_heads_off: self.o_byte_heads_off,
            o_vals_off: self.o_vals_off,
            o_idx: 0,
            o_end: 0,
            o_byte_off: 0,
            current_o: 0,
            need_o_first: true,
        }
    }
    /// Iterate `(S, O)` pairs across every block.
    pub fn iter_all(&self) -> impl Iterator<Item = (u64, u64, u64)> + '_ {
        (0..self.n_gids as usize).flat_map(move |bi| {
            let gid = self.u32_at(self.gid_vals_off, bi) as u64;
            self.iter_block(bi).map(move |(s, o)| (gid, s, o))
        })
    }
}

/// Iterator over `(S, O)` (or `(O, S)`) pairs within one block (one gid).
pub struct IdxBlockIter<'a> {
    bytes: &'a [u8],
    s_idx: usize,
    s_end: usize,
    s_byte_off: usize,
    current_s: u64,
    need_s: bool,
    o_heads_off: usize,
    o_byte_heads_off: usize,
    o_vals_off: usize,
    o_idx: usize,
    o_end: usize,
    o_byte_off: usize,
    current_o: u64,
    need_o_first: bool,
}

impl<'a> Iterator for IdxBlockIter<'a> {
    type Item = (u64, u64);
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Advance to next S run if needed.
            while self.o_idx >= self.o_end {
                if self.s_idx >= self.s_end {
                    return None;
                }
                // Decode next S
                if self.need_s {
                    let (v, n) = read_uvarint(self.bytes, self.s_byte_off)?;
                    self.s_byte_off = n;
                    self.current_s = v;
                    self.need_s = false;
                } else {
                    let (d, n) = read_uvarint(self.bytes, self.s_byte_off)?;
                    self.s_byte_off = n;
                    self.current_s = self.current_s.checked_add(d)?;
                }
                // Lookup o run
                let o_start_off = self.o_heads_off + self.s_idx * 4;
                let o_end_off = o_start_off + 4;
                let o_start = u32::from_le_bytes(self.bytes[o_start_off..o_start_off + 4].try_into().unwrap())
                    as usize;
                let o_end =
                    u32::from_le_bytes(self.bytes[o_end_off..o_end_off + 4].try_into().unwrap()) as usize;
                self.o_idx = o_start;
                self.o_end = o_end;
                if o_end > o_start {
                    let oboff_off = self.o_byte_heads_off + self.s_idx * 4;
                    let oboff =
                        u32::from_le_bytes(self.bytes[oboff_off..oboff_off + 4].try_into().unwrap())
                            as usize;
                    self.o_byte_off = self.o_vals_off + oboff;
                    self.need_o_first = true;
                }
                self.s_idx += 1;
            }
            // Decode next O
            if self.need_o_first {
                let (v, n) = read_uvarint(self.bytes, self.o_byte_off)?;
                self.o_byte_off = n;
                self.current_o = v;
                self.need_o_first = false;
            } else {
                let (d, n) = read_uvarint(self.bytes, self.o_byte_off)?;
                self.o_byte_off = n;
                self.current_o = self.current_o.checked_add(d)?;
            }
            self.o_idx += 1;
            return Some((self.current_s, self.current_o));
        }
    }
}

/// One parsed permutation section.
#[derive(Debug, Clone)]
pub struct IdxSection {
    pub kind: IdxKind,
    pub off: u64,
    pub len: u64,
    n_predicates: u64,
    pred_keys_off: usize,
    key2post_offs_off: usize,
    blob_off: usize,
}

impl IdxSection {
    /// Binary-search for a predicate term-id and return its posting view.
    pub fn lookup<'a>(&self, bytes: &'a [u8], p_id: u64) -> Option<IdxPosting<'a>> {
        let n = self.n_predicates as usize;
        if n == 0 {
            return None;
        }
        let mut lo = 0usize;
        let mut hi = n;
        let mut found: Option<usize> = None;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let off = self.pred_keys_off + mid * 8;
            let v = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
            if v == p_id {
                found = Some(mid);
                break;
            } else if v < p_id {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let idx = found?;
        let pe = self.key2post_offs_off + idx * 8;
        let s_off = u64::from_le_bytes(bytes[pe..pe + 8].try_into().unwrap()) as usize;
        let e_off =
            u64::from_le_bytes(bytes[pe + 8..pe + 16].try_into().unwrap()) as usize;
        let post_start = self.blob_off + s_off;
        let post_end = self.blob_off + e_off;
        parse_posting(&bytes[post_start..post_end])
    }
}

fn parse_posting(payload: &[u8]) -> Option<IdxPosting<'_>> {
    // Offsets are payload-relative; the returned IdxPosting borrows from the
    // posting slice directly.
    if payload.len() < 4 {
        return None;
    }
    let n_gids = u32::from_le_bytes(payload[0..4].try_into().ok()?);
    let mut off = 4usize;
    let gid_vals_off = off;
    off = off.checked_add((n_gids as usize) * 4)?;
    let s_heads_off = off;
    off = off.checked_add((n_gids as usize + 1) * 4)?;
    let s_byte_heads_off = off;
    off = off.checked_add((n_gids as usize + 1) * 4)?;
    if off + 4 > payload.len() {
        return None;
    }
    let n_s = u32::from_le_bytes(payload[off..off + 4].try_into().ok()?);
    off += 4;
    let s_vals_off = off;
    // We don't know the byte length of s_vals upfront; locate o_heads via
    // s_byte_heads[n_gids] which is the total byte length of s_vals.
    let s_vals_byte_len =
        u32::from_le_bytes(payload[s_byte_heads_off + (n_gids as usize) * 4..s_byte_heads_off + (n_gids as usize) * 4 + 4].try_into().ok()?) as usize;
    off = off.checked_add(s_vals_byte_len)?;
    let o_heads_off = off;
    off = off.checked_add((n_s as usize + 1) * 4)?;
    let o_byte_heads_off = off;
    off = off.checked_add((n_s as usize + 1) * 4)?;
    if off + 4 > payload.len() {
        return None;
    }
    let n_t = u32::from_le_bytes(payload[off..off + 4].try_into().ok()?);
    off += 4;
    let o_vals_off = off;
    Some(IdxPosting {
        bytes: payload,
        n_gids,
        gid_vals_off,
        s_heads_off,
        s_byte_heads_off,
        n_s,
        s_vals_off,
        o_heads_off,
        o_byte_heads_off,
        n_t,
        o_vals_off,
    })
}

/// One side (forward or reverse) of a precomputed transitive-closure index.
///
/// `keys` is a sorted ascending array of `u64` term IDs. `heads` is a prefix-sum
/// array (length `n_keys + 1`) indexing into `vals`. For key at index `i`, the
/// sorted reachable set is `vals[heads[i]..heads[i + 1]]`.
#[derive(Debug, Clone)]
pub struct PClosSide<'a> {
    n_keys: u32,
    keys: &'a [u8], // [u64 * n_keys] little-endian
    heads: &'a [u8], // [u32 * (n_keys + 1)] little-endian
    vals: &'a [u8], // [u64 * n_vals] little-endian
}

impl<'a> PClosSide<'a> {
    pub fn n_keys(&self) -> u32 {
        self.n_keys
    }

    fn key_at(&self, i: usize) -> u64 {
        let off = i * 8;
        u64::from_le_bytes(self.keys[off..off + 8].try_into().unwrap())
    }
    fn head_at(&self, i: usize) -> u32 {
        let off = i * 4;
        u32::from_le_bytes(self.heads[off..off + 4].try_into().unwrap())
    }

    /// Returns the reachable set (sorted, no duplicates) for `key`, or
    /// `None` when the key has no precomputed closure (no outgoing edges
    /// under this predicate).
    pub fn lookup(&self, key: u64) -> Option<Vec<u64>> {
        let n = self.n_keys as usize;
        if n == 0 {
            return None;
        }
        let mut lo = 0usize;
        let mut hi = n;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let v = self.key_at(mid);
            if v == key {
                let start = self.head_at(mid) as usize;
                let end = self.head_at(mid + 1) as usize;
                let mut out = Vec::with_capacity(end - start);
                for i in start..end {
                    let off = i * 8;
                    out.push(u64::from_le_bytes(self.vals[off..off + 8].try_into().unwrap()));
                }
                return Some(out);
            } else if v < key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        None
    }
}

/// Parsed `IDX_PCLOS` section. Holds zero-copy slice references into the
/// sidecar's mmap so per-predicate lookups don't allocate beyond the
/// final answer set.
#[derive(Debug, Clone)]
pub struct PClosSection {
    pub off: u64,
    pub len: u64,
    n_predicates: u64,
    pred_keys_off: usize,
    // Per-predicate posting bounds, laid out as 4 × u64 each:
    //   [fwd_off, fwd_len, rev_off, rev_len]
    entries_off: usize,
}

impl PClosSection {
    fn pred_at(&self, bytes: &[u8], i: usize) -> u64 {
        let off = self.pred_keys_off + i * 8;
        u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap())
    }

    fn find_predicate(&self, bytes: &[u8], p_id: u64) -> Option<usize> {
        let n = self.n_predicates as usize;
        if n == 0 {
            return None;
        }
        let mut lo = 0usize;
        let mut hi = n;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let v = self.pred_at(bytes, mid);
            if v == p_id {
                return Some(mid);
            } else if v < p_id {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        None
    }

    fn side<'a>(&self, bytes: &'a [u8], idx: usize, reverse: bool) -> Option<PClosSide<'a>> {
        let off = self.entries_off + idx * 32;
        let fwd_off = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap()) as usize;
        let fwd_len = u64::from_le_bytes(bytes[off + 8..off + 16].try_into().unwrap()) as usize;
        let rev_off = u64::from_le_bytes(bytes[off + 16..off + 24].try_into().unwrap()) as usize;
        let rev_len = u64::from_le_bytes(bytes[off + 24..off + 32].try_into().unwrap()) as usize;
        let (posting_off, posting_len) = if reverse {
            (rev_off, rev_len)
        } else {
            (fwd_off, fwd_len)
        };
        if posting_len == 0 {
            return None;
        }
        parse_pclos_side(&bytes[posting_off..posting_off + posting_len])
    }

    /// Forward closure: subject -> sorted set of reachable objects under `p_id`.
    pub fn forward<'a>(&self, bytes: &'a [u8], p_id: u64) -> Option<PClosSide<'a>> {
        let idx = self.find_predicate(bytes, p_id)?;
        self.side(bytes, idx, false)
    }

    /// Reverse closure: object -> sorted set of subjects that reach it
    /// transitively under `p_id`.
    pub fn reverse<'a>(&self, bytes: &'a [u8], p_id: u64) -> Option<PClosSide<'a>> {
        let idx = self.find_predicate(bytes, p_id)?;
        self.side(bytes, idx, true)
    }
}

fn parse_pclos_side(payload: &[u8]) -> Option<PClosSide<'_>> {
    if payload.len() < 8 {
        return None;
    }
    let n_keys = u32::from_le_bytes(payload[0..4].try_into().ok()?);
    let n_vals = u32::from_le_bytes(payload[4..8].try_into().ok()?);
    let keys_off = 8usize;
    let keys_size = (n_keys as usize).checked_mul(8)?;
    let heads_off = keys_off.checked_add(keys_size)?;
    let heads_size = (n_keys as usize + 1).checked_mul(4)?;
    let vals_off = heads_off.checked_add(heads_size)?;
    let vals_size = (n_vals as usize).checked_mul(8)?;
    if vals_off.checked_add(vals_size)? > payload.len() {
        return None;
    }
    Some(PClosSide {
        n_keys,
        keys: &payload[keys_off..keys_off + keys_size],
        heads: &payload[heads_off..heads_off + heads_size],
        vals: &payload[vals_off..vals_off + vals_size],
    })
}

// ---------------- Writer ----------------

fn write_uvarint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

// ============================================================================
// In-memory index builders
// ============================================================================

/// An in-memory, owned index for one permutation, built directly from an
/// [`R5tuFile`]. Holds the same compact posting layout the on-disk sidecar
/// used; query with [`MemSection::lookup`]. The byte buffer's offsets are
/// relative to its own start (base 0), so it is self-contained.
#[derive(Debug)]
pub struct MemSection {
    kind: IdxKind,
    bytes: Vec<u8>,
    section: IdxSection,
}

impl MemSection {
    /// The permutation this section indexes.
    pub fn kind(&self) -> IdxKind {
        self.kind
    }

    /// Look up the posting for `key` (a subject id for SPO, object id for OSP,
    /// predicate id for PSO/POS). The posting's `(A, B)` pairs follow the
    /// permutation's convention — see [`build_section`].
    pub fn lookup(&self, key: u64) -> Option<IdxPosting<'_>> {
        self.section.lookup(&self.bytes, key)
    }
}

/// Parse a freshly-built (base-0) section buffer into an [`IdxSection`] view.
fn parse_mem_section(kind: IdxKind, bytes: &[u8]) -> Result<IdxSection> {
    if bytes.len() < 32 {
        return Err(R5Error::Corrupt("mem section too short".into()));
    }
    let n_predicates = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    let pred_keys_off = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
    let key2post_offs_off = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
    let blob_off = u64::from_le_bytes(bytes[24..32].try_into().unwrap()) as usize;
    Ok(IdxSection {
        kind,
        off: 0,
        len: bytes.len() as u64,
        n_predicates,
        pred_keys_off,
        key2post_offs_off,
        blob_off,
    })
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

/// Build one permutation index in memory from an open snapshot. Accepts
/// `Pso`, `Pos`, `Spo`, or `Osp`; use [`build_mem_pclos`] for `PClos`.
pub fn build_mem_section(r5tu: &R5tuFile, kind: IdxKind) -> Result<MemSection> {
    let mut tuples = collect_tuples(r5tu)?;
    let bytes = match kind {
        IdxKind::Pso => {
            tuples.sort_unstable(); // (p, gid, s, o)
            build_section(&tuples, /*swap_s_o=*/ false)
        }
        IdxKind::Pos => {
            tuples.sort_unstable_by(|a, b| {
                a.0.cmp(&b.0)
                    .then(a.1.cmp(&b.1))
                    .then(a.3.cmp(&b.3))
                    .then(a.2.cmp(&b.2))
            }); // (p, gid, o, s)
            build_section(&tuples, /*swap_s_o=*/ true)
        }
        IdxKind::Spo => {
            let mut t: Vec<(u64, u32, u64, u64)> =
                tuples.iter().map(|&(p, g, s, o)| (s, g, p, o)).collect();
            t.sort_unstable();
            build_section(&t, /*swap_s_o=*/ false)
        }
        IdxKind::Osp => {
            let mut t: Vec<(u64, u32, u64, u64)> =
                tuples.iter().map(|&(p, g, s, o)| (o, g, s, p)).collect();
            t.sort_unstable();
            build_section(&t, /*swap_s_o=*/ false)
        }
        IdxKind::PClos => {
            return Err(R5Error::Invalid("use build_mem_pclos for PClos"));
        }
    };
    let section = parse_mem_section(kind, &bytes)?;
    Ok(MemSection { kind, bytes, section })
}

/// An in-memory precomputed transitive-closure index over a configured set of
/// predicates, built directly from an [`R5tuFile`].
#[derive(Debug)]
pub struct MemPClos {
    bytes: Vec<u8>,
    section: PClosSection,
}

impl MemPClos {
    /// Forward closure: objects reachable from `subject` via one or more
    /// `predicate` edges, sorted; `None` if `predicate` has no precomputed
    /// closure or `subject` has no outgoing edges under it.
    pub fn closure_forward(&self, predicate: u64, subject: u64) -> Option<Vec<u64>> {
        self.section.forward(&self.bytes, predicate)?.lookup(subject)
    }

    /// Reverse closure: subjects that reach `object` via one or more
    /// `predicate` edges, sorted.
    pub fn closure_reverse(&self, predicate: u64, object: u64) -> Option<Vec<u64>> {
        self.section.reverse(&self.bytes, predicate)?.lookup(object)
    }
}

/// Build the in-memory transitive-closure index for `predicates`.
pub fn build_mem_pclos(r5tu: &R5tuFile, predicates: &[u64]) -> Result<MemPClos> {
    // build_pclos_section reads (p, gid, s, o) tuples directly (fields .2 = s,
    // .3 = o); it groups by predicate itself, so no particular sort is needed.
    let tuples = collect_tuples(r5tu)?;
    let bytes = build_pclos_section(&tuples, predicates);
    if bytes.len() < 24 {
        return Err(R5Error::Corrupt("mem pclos too short".into()));
    }
    let n_predicates = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    let pred_keys_off = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
    let entries_off = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
    let section = PClosSection {
        off: 0,
        len: bytes.len() as u64,
        n_predicates,
        pred_keys_off,
        entries_off,
    };
    Ok(MemPClos { bytes, section })
}

/// Build one permutation section. `tuples` must be sorted by
/// `(key, gid, A, B)`. When `swap_s_o` is false the posting's inner pairs are
/// `(A, B)` as given; when true the tuple was sorted with its last two fields
/// swapped, so the posting emits `(B, A)`.
///
/// The key dimension and the meaning of `A`/`B` depend on the permutation:
///   PSO: key = p, A = s, B = o      POS: key = p, A = o, B = s (swap)
///   SPO: key = s, A = p, B = o      OSP: key = o, A = s, B = p
fn build_section(tuples: &[(u64, u32, u64, u64)], swap_s_o: bool) -> Vec<u8> {
    // Group by predicate
    let mut postings: Vec<(u64, Vec<u8>)> = Vec::new();
    if tuples.is_empty() {
        return serialize_section(&postings);
    }

    let mut i = 0usize;
    while i < tuples.len() {
        let p = tuples[i].0;
        let mut j = i;
        while j < tuples.len() && tuples[j].0 == p {
            j += 1;
        }
        let block = build_posting_block(&tuples[i..j], swap_s_o);
        postings.push((p, block));
        i = j;
    }
    serialize_section(&postings)
}

fn build_posting_block(slice: &[(u64, u32, u64, u64)], swap_s_o: bool) -> Vec<u8> {
    // slice has constant p; iterate by gid.
    // Determine which fields are "A" (delta'd outer inner) and "B".
    let get_ab = |t: &(u64, u32, u64, u64)| -> (u64, u64) {
        if swap_s_o {
            // POS: tuples were sorted (p, gid, o, s) -> tuple is (p, gid, s_orig, o_orig)?
            // Look at how callers sort:
            //   PSO: sort by (p, gid, s, o) — tuples are (p, gid, s, o), A=s, B=o.
            //   POS: sort by .0,.1,.3,.2  (p, gid, o, s) — tuples are still (p, gid, s, o),
            //        so A = o = .3, B = s = .2
            (t.3, t.2)
        } else {
            (t.2, t.3)
        }
    };

    // Walk gids
    let mut gid_vals: Vec<u32> = Vec::new();
    let mut s_heads: Vec<u32> = vec![0]; // prefix sums into s_vals (element counts)
    let mut s_byte_heads: Vec<u32> = vec![0]; // byte offsets into s_vals stream
    let mut s_vals_bytes: Vec<u8> = Vec::new();
    let mut o_heads: Vec<u32> = vec![0]; // prefix sums into o_vals (element counts)
    let mut o_byte_heads: Vec<u32> = vec![0]; // byte offsets into o_vals stream
    let mut o_vals_bytes: Vec<u8> = Vec::new();
    let mut n_s_total: u32 = 0;
    let mut n_t_total: u32 = 0;

    let mut i = 0usize;
    while i < slice.len() {
        let gid = slice[i].1;
        let mut j = i;
        while j < slice.len() && slice[j].1 == gid {
            j += 1;
        }
        gid_vals.push(gid);
        // Within this gid: group by A (inner-outer)
        let mut prev_a: Option<u64> = None;
        let mut s_count_in_gid: u32 = 0;
        let mut k = i;
        while k < j {
            let (a_k, _) = get_ab(&slice[k]);
            let mut l = k;
            while l < j && get_ab(&slice[l]).0 == a_k {
                l += 1;
            }
            // Emit S value (delta within the gid run).
            let delta = match prev_a {
                None => a_k,
                Some(prev) => a_k - prev,
            };
            write_uvarint(&mut s_vals_bytes, delta);
            prev_a = Some(a_k);

            // Emit O run for this A.
            let mut prev_b: Option<u64> = None;
            for m in k..l {
                let (_, b_m) = get_ab(&slice[m]);
                let dlt = match prev_b {
                    None => b_m,
                    Some(p) => b_m - p,
                };
                write_uvarint(&mut o_vals_bytes, dlt);
                prev_b = Some(b_m);
                n_t_total += 1;
            }
            n_s_total += 1;
            s_count_in_gid += 1;
            // Record o_heads and o_byte_heads for this S row.
            o_heads.push(n_t_total);
            o_byte_heads.push(o_vals_bytes.len() as u32);
            k = l;
        }
        // After processing this gid:
        s_heads.push(n_s_total);
        s_byte_heads.push(s_vals_bytes.len() as u32);
        let _ = s_count_in_gid;
        i = j;
    }

    // Now serialize posting:
    let mut out = Vec::new();
    out.extend_from_slice(&(gid_vals.len() as u32).to_le_bytes());
    for g in &gid_vals {
        out.extend_from_slice(&g.to_le_bytes());
    }
    for h in &s_heads {
        out.extend_from_slice(&h.to_le_bytes());
    }
    for h in &s_byte_heads {
        out.extend_from_slice(&h.to_le_bytes());
    }
    out.extend_from_slice(&n_s_total.to_le_bytes());
    out.extend_from_slice(&s_vals_bytes);
    for h in &o_heads {
        out.extend_from_slice(&h.to_le_bytes());
    }
    for h in &o_byte_heads {
        out.extend_from_slice(&h.to_le_bytes());
    }
    out.extend_from_slice(&n_t_total.to_le_bytes());
    out.extend_from_slice(&o_vals_bytes);
    out
}

fn serialize_section(postings: &[(u64, Vec<u8>)]) -> Vec<u8> {
    // Section header: u64 n_predicates, u64 pred_keys_off, u64 key2post_offs_off, u64 blob_off
    // Then pred_keys[n_predicates], key2post_offs[n_predicates+1] (byte offsets into blob),
    // then blob (concatenated postings).
    let n = postings.len() as u64;
    // We need absolute file offsets, but we don't know where this section sits in the
    // larger file yet. Patch them as relative-from-section-start, then add base_off
    // when writing into output. We'll write the header with absolute placeholders and
    // patch later? No — produce the section as a self-contained buffer and have the
    // outer writer record offsets that include the section's base offset.
    //
    // Strategy: produce a buffer with the offsets stored as ABSOLUTE assuming the
    // section will be placed at offset 0. Then in the outer writer, we patch each
    // u64 offset by adding the actual base offset. Use the layout:
    //
    //   [0]  n_predicates u64
    //   [8]  pred_keys_off u64           — patch later
    //   [16] key2post_offs_off u64       — patch later
    //   [24] blob_off u64                — patch later
    //   [32] pred_keys[n]
    //   [32 + 8n] key2post_offs[n+1] (u64 byte offsets into blob, relative)
    //   [32 + 8n + 8(n+1)] blob (concatenated)
    //
    // We'll return the buffer and let the outer writer call `patch_section_offsets`
    // with the right base.
    let header_size = 32usize;
    let pred_keys_size = 8 * postings.len();
    let key2post_size = 8 * (postings.len() + 1);
    let mut out = Vec::with_capacity(header_size + pred_keys_size + key2post_size);
    // Header — placeholder. We'll patch with actual relative offsets.
    out.extend_from_slice(&n.to_le_bytes());
    let pred_keys_off_rel = header_size as u64;
    let key2post_off_rel = (header_size + pred_keys_size) as u64;
    let blob_off_rel = (header_size + pred_keys_size + key2post_size) as u64;
    out.extend_from_slice(&pred_keys_off_rel.to_le_bytes());
    out.extend_from_slice(&key2post_off_rel.to_le_bytes());
    out.extend_from_slice(&blob_off_rel.to_le_bytes());
    // pred_keys
    for (p, _) in postings {
        out.extend_from_slice(&p.to_le_bytes());
    }
    // key2post_offs (relative to blob)
    let mut cum: u64 = 0;
    out.extend_from_slice(&cum.to_le_bytes());
    for (_, b) in postings {
        cum += b.len() as u64;
        out.extend_from_slice(&cum.to_le_bytes());
    }
    // blob
    for (_, b) in postings {
        out.extend_from_slice(b);
    }
    // Now the section is self-contained but offsets are RELATIVE to section start.
    // The reader's parse_section reads them and treats them as absolute file
    // offsets. So before assembling the final file, we must patch them by adding
    // the section's actual base offset within the file.
    out
}

// ============================================================================
// PClos: precomputed transitive-closure index
// ============================================================================
//
// Section layout (offsets relative to the section's base, patched to
// absolute file offsets via `patch_pclos_offsets` after placement):
//
//   u64 n_predicates
//   u64 pred_keys_off    -> [u64 * n_predicates] sorted predicate IDs
//   u64 entries_off      -> [PClosEntry * n_predicates]
//
// PClosEntry (32 bytes):
//   u64 fwd_off  u64 fwd_len  u64 rev_off  u64 rev_len    (absolute file offsets)
//
// Each forward/reverse posting:
//   u32 n_keys
//   u32 n_vals
//   [u64 * n_keys]      keys, sorted ascending
//   [u32 * (n_keys+1)]  heads, prefix sums into vals
//   [u64 * n_vals]      vals, sorted ascending within each key's slice
//
// Cycle handling: BFS uses a visited set, so cycles produce a closure
// containing the participating nodes and terminate. The closure is the
// non-reflexive transitive closure; callers who want reflexive (`P*`)
// semantics add the source node themselves.

fn build_pclos_section(
    pos_tuples: &[(u64, u32, u64, u64)],
    closure_predicates: &[u64],
) -> Vec<u8> {
    // Section is built in two stages: per-predicate postings as
    // self-contained buffers (with placeholder offsets), then a header
    // wrapping them. Offsets inside the per-predicate entries are
    // patched to absolute file offsets via `patch_pclos_offsets`
    // after the section's base offset is known.

    // Sort predicates ascending so the lookup binary search works.
    let mut preds: Vec<u64> = closure_predicates.to_vec();
    preds.sort_unstable();
    preds.dedup();

    // For each predicate, gather the direct adjacency from pos_tuples
    // (which is sorted by (p, gid, o, s) — we want (p, s, o) for the
    // forward BFS, so we iterate and re-collect).
    let mut postings: Vec<(u64, Vec<u8>, Vec<u8>)> = Vec::new(); // (p, fwd_bytes, rev_bytes)
    for p in &preds {
        // Collect distinct (s, o) pairs for predicate p, ignoring gid (the
        // closure is whole-snapshot, not per-graph).
        let mut adjacency: std::collections::BTreeMap<u64, std::collections::BTreeSet<u64>> =
            std::collections::BTreeMap::new();
        for &(tp, _gid, s_or_o_outer, s_or_o_inner) in pos_tuples {
            if tp != *p {
                continue;
            }
            // In pos_tuples (sorted by p, gid, o, s), .2 is the original
            // subject and .3 is the original object — see build_section.
            let s = s_or_o_outer;
            let o = s_or_o_inner;
            adjacency.entry(s).or_default().insert(o);
        }
        if adjacency.is_empty() {
            postings.push((*p, Vec::new(), Vec::new()));
            continue;
        }
        // Forward closure: for each subject, BFS in `adjacency`.
        let fwd_bytes = serialize_pclos_posting(bfs_closure_table(&adjacency));
        // Reverse closure: invert and BFS.
        let mut inv: std::collections::BTreeMap<u64, std::collections::BTreeSet<u64>> =
            std::collections::BTreeMap::new();
        for (s, os) in &adjacency {
            for o in os {
                inv.entry(*o).or_default().insert(*s);
            }
        }
        let rev_bytes = serialize_pclos_posting(bfs_closure_table(&inv));
        postings.push((*p, fwd_bytes, rev_bytes));
    }

    serialize_pclos_section(&postings)
}

fn bfs_closure_table(
    adj: &std::collections::BTreeMap<u64, std::collections::BTreeSet<u64>>,
) -> std::collections::BTreeMap<u64, Vec<u64>> {
    // For every node that appears as a key (source) in `adj`, compute the
    // set of nodes reachable via one or more steps. Iterative BFS with a
    // visited set; safe against cycles.
    let mut out = std::collections::BTreeMap::new();
    for start in adj.keys() {
        let mut visited = std::collections::BTreeSet::new();
        let mut queue: std::collections::VecDeque<u64> = std::collections::VecDeque::new();
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
        if visited.is_empty() {
            continue;
        }
        out.insert(*start, visited.into_iter().collect());
    }
    out
}

fn serialize_pclos_posting(
    table: std::collections::BTreeMap<u64, Vec<u64>>,
) -> Vec<u8> {
    if table.is_empty() {
        return Vec::new();
    }
    let n_keys = table.len() as u32;
    let n_vals: u32 = table.values().map(|v| v.len() as u32).sum();
    let mut out = Vec::with_capacity(
        8 + (n_keys as usize) * 8 + (n_keys as usize + 1) * 4 + (n_vals as usize) * 8,
    );
    out.extend_from_slice(&n_keys.to_le_bytes());
    out.extend_from_slice(&n_vals.to_le_bytes());
    // keys
    for k in table.keys() {
        out.extend_from_slice(&k.to_le_bytes());
    }
    // heads (prefix sums)
    let mut sum: u32 = 0;
    out.extend_from_slice(&sum.to_le_bytes());
    for v in table.values() {
        sum += v.len() as u32;
        out.extend_from_slice(&sum.to_le_bytes());
    }
    // vals
    for v in table.values() {
        for x in v {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
    out
}

fn serialize_pclos_section(postings: &[(u64, Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    // Header layout (24 bytes):
    //   u64 n_predicates
    //   u64 pred_keys_off  (relative; patched later)
    //   u64 entries_off    (relative; patched later)
    let n = postings.len() as u64;
    let header_size = 24usize;
    let pred_keys_size = (postings.len()) * 8;
    let entries_size = (postings.len()) * 32;
    // Pred keys at [header_size]
    // Entries at [header_size + pred_keys_size]
    // Blob at [header_size + pred_keys_size + entries_size]
    let blob_off_local = header_size + pred_keys_size + entries_size;

    let mut out = Vec::new();
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&(header_size as u64).to_le_bytes());
    out.extend_from_slice(
        &((header_size + pred_keys_size) as u64).to_le_bytes(),
    );
    // pred keys
    for (p, _, _) in postings {
        out.extend_from_slice(&p.to_le_bytes());
    }
    // entries — write placeholders for absolute offsets, patch after blob known.
    let entries_start = out.len();
    out.resize(entries_start + entries_size, 0);
    // blob
    let mut blob_cur = blob_off_local as u64;
    let mut entry_writes: Vec<(u64, u64, u64, u64)> = Vec::with_capacity(postings.len());
    for (_, fwd, rev) in postings {
        let fwd_off_rel = if fwd.is_empty() { 0 } else { blob_cur };
        let fwd_len = fwd.len() as u64;
        blob_cur += fwd_len;
        let rev_off_rel = if rev.is_empty() { 0 } else { blob_cur };
        let rev_len = rev.len() as u64;
        blob_cur += rev_len;
        entry_writes.push((fwd_off_rel, fwd_len, rev_off_rel, rev_len));
        out.extend_from_slice(fwd);
        out.extend_from_slice(rev);
    }
    // Write entries
    for (i, (fwd_off, fwd_len, rev_off, rev_len)) in entry_writes.iter().enumerate() {
        let off = entries_start + i * 32;
        out[off..off + 8].copy_from_slice(&fwd_off.to_le_bytes());
        out[off + 8..off + 16].copy_from_slice(&fwd_len.to_le_bytes());
        out[off + 16..off + 24].copy_from_slice(&rev_off.to_le_bytes());
        out[off + 24..off + 32].copy_from_slice(&rev_len.to_le_bytes());
    }
    out
}

fn read_uvarint(buf: &[u8], mut off: usize) -> Option<(u64, usize)> {
    let (mut x, mut s) = (0u64, 0u32);
    for _ in 0..10 {
        let b = *buf.get(off)? as u64;
        off += 1;
        x |= (b & 0x7f) << s;
        if b & 0x80 == 0 {
            return Some((x, off));
        }
        s += 7;
    }
    None
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
