//! Reader for R5TU files: open, inspect sections, and iterate triples.
//!
//! The primary entry point is [`R5tuFile`]. Use it to open a `.r5tu`
//! file and query logical graph groups by dataset id and graph name.
//! See `ARCH.md` §4 for query semantics and identifiers.
//!
//! Basic example
//!
//! ```no_run
//! use rdf5d::R5tuFile;
//! use std::path::Path;
//!
//! let f = R5tuFile::open(Path::new("example.r5tu")).expect("open");
//! if let Some(gr) = f.resolve_gid("dataset:1", "http://example.org/graph").unwrap() {
//!     let mut n = 0u64;
//!     for (s, p, o) in f.triples_ids(gr.gid).unwrap() { n += 1; }
//!     assert_eq!(n, gr.n_triples);
//! }
//! ```

use std::{fmt, fs, path::Path};

use crate::header::{
    Header, Section, SectionKind, TocEntry, crc32_ieee, parse_footer, parse_toc, section_in_bounds,
};

/// Errors that can arise when parsing or validating an R5TU file.
#[derive(Debug)]
pub enum R5Error {
    /// Underlying I/O error.
    Io(std::io::Error),
    /// Structural problem with inputs or unsupported feature.
    Invalid(&'static str),
    /// The file failed an integrity or bounds check.
    Corrupt(String),
}

impl fmt::Display for R5Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            R5Error::Io(e) => write!(f, "{}", e),
            R5Error::Invalid(m) => write!(f, "{}", m),
            R5Error::Corrupt(m) => write!(f, "{}", m),
        }
    }
}
impl std::error::Error for R5Error {}
impl From<std::io::Error> for R5Error {
    fn from(e: std::io::Error) -> Self {
        R5Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, R5Error>;

/// Lightweight description of a logical graph group inside an R5TU file.
#[derive(Debug, Clone)]
pub struct GraphRef {
    /// Stable group id within the file.
    pub gid: u64,
    /// Dataset identifier (id) for the group (as stored in the id dictionary).
    pub id: String,
    /// Graph name (as stored in the graph name dictionary).
    pub graphname: String,
    /// Number of triples in this group.
    pub n_triples: u64,
}

#[derive(Debug)]
enum Backing {
    Owned(Vec<u8>),
    #[cfg(feature = "mmap")]
    Mmap(memmap2::Mmap),
}

impl Backing {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Backing::Owned(v) => v.as_slice(),
            #[cfg(feature = "mmap")]
            Backing::Mmap(m) => m,
        }
    }
}

/// Opened R5TU file. Provides lookups and triple iteration.
#[derive(Debug)]
pub struct R5tuFile {
    backing: Backing,
    header: Header,
    toc: Vec<TocEntry>,
    // sections
    id_dict: Dict,
    gname_dict: Dict,
    term_dict: TermDict,
    gdir: Section,
    idx_id2gid: Section,
    idx_gname2gid: Section,
    idx_pair2gid: Section,
    #[allow(dead_code)]
    triple_blocks: Section,
}

impl R5tuFile {
    #[inline]
    fn bytes(&self) -> &[u8] {
        self.backing.as_bytes()
    }
    /// Open and validate an R5TU file from disk.
    ///
    /// Performs bounds checks, TOC validation, and optional section/global CRCs.
    /// Returns a handle capable of dictionary lookups and triple iteration.
    pub fn open(path: &Path) -> Result<Self> {
        let data = fs::read(path)?;
        let header = Header::parse(&data).ok_or(R5Error::Invalid("short or invalid header"))?;
        if &header.magic != b"R5TU" {
            return Err(R5Error::Invalid("bad magic"));
        }
        // basic header sanity
        if header.toc_off_u64 as usize > data.len() {
            return Err(R5Error::Corrupt("TOC offset out of bounds".into()));
        }
        let toc =
            parse_toc(&data, &header).ok_or_else(|| R5Error::Corrupt("TOC parse failed".into()))?;
        // verify sections lie within file and optional CRCs
        for e in &toc {
            if !section_in_bounds(data.len(), e.section) {
                return Err(R5Error::Corrupt(format!(
                    "section {:?} out of bounds",
                    e.kind
                )));
            }
            if e.crc32_u32 != 0 {
                let start = e.section.off as usize;
                let end = start + e.section.len as usize;
                if end > data.len() {
                    return Err(R5Error::Corrupt("section crc OOB".into()));
                }
                let got = crc32_ieee(&data[start..end]);
                if got != e.crc32_u32 {
                    return Err(R5Error::Corrupt("section CRC mismatch".into()));
                }
            }
        }
        // Validate TOC ordering by offset and detect overlaps
        let mut spans: Vec<(u64, u64)> =
            toc.iter().map(|e| (e.section.off, e.section.len)).collect();
        spans.sort_by_key(|(off, _)| *off);
        for w in spans.windows(2) {
            let (a_off, a_len) = w[0];
            let (b_off, _b_len) = w[1];
            if a_off + a_len > b_off {
                return Err(R5Error::Corrupt("TOC sections overlap or unsorted".into()));
            }
        }
        // Resolve required sections
        let need = |k: SectionKind| -> Result<Section> {
            parse_toc(&data, &header)
                .and_then(|t| t.into_iter().find(|e| e.kind == k).map(|e| e.section))
                .ok_or(R5Error::Invalid("missing required section"))
        };
        let id_sec = need(SectionKind::IdDict)?;
        let gn_sec = need(SectionKind::GNameDict)?;
        let term_sec = need(SectionKind::TermDict)?;
        let gdir = need(SectionKind::GDir)?;
        let idx_id2gid = need(SectionKind::IdxId2Gid)?;
        let idx_gname2gid = need(SectionKind::IdxGName2Gid)?;
        let idx_pair2gid = need(SectionKind::IdxPair2Gid)?;
        let triple_blocks = need(SectionKind::TripleBlocks)?;

        let id_dict = Dict::parse(&data, id_sec)?;
        let gname_dict = Dict::parse(&data, gn_sec)?;
        let term_dict = TermDict::parse(&data, term_sec)?;

        // Footer/global CRC
        if let Some((footer_crc, magic)) = parse_footer(&data) {
            if &magic != b"R5TU_ENDMARK" {
                return Err(R5Error::Corrupt("bad footer magic".into()));
            }
            let got = crc32_ieee(&data[..data.len() - 16]);
            if got != footer_crc {
                return Err(R5Error::Corrupt("global CRC mismatch".into()));
            }
        }
        Ok(Self {
            backing: Backing::Owned(data),
            header,
            toc,
            id_dict,
            gname_dict,
            term_dict,
            gdir,
            idx_id2gid,
            idx_gname2gid,
            idx_pair2gid,
            triple_blocks,
        })
    }

    #[cfg(feature = "mmap")]
    /// Open and validate an R5TU file using `memmap2` for zero‑copy access.
    ///
    /// Enabled with the `mmap` feature.
    pub fn open_mmap(path: &Path) -> Result<Self> {
        use std::fs::File;
        let f = File::open(path)?;
        let mmap = unsafe { memmap2::MmapOptions::new().map(&f) }.map_err(R5Error::Io)?;
        let data: &[u8] = &mmap;
        let header = Header::parse(data).ok_or(R5Error::Invalid("short or invalid header"))?;
        if &header.magic != b"R5TU" {
            return Err(R5Error::Invalid("bad magic"));
        }
        if header.toc_off_u64 as usize > data.len() {
            return Err(R5Error::Corrupt("TOC offset out of bounds".into()));
        }
        let toc =
            parse_toc(data, &header).ok_or_else(|| R5Error::Corrupt("TOC parse failed".into()))?;
        for e in &toc {
            if !section_in_bounds(data.len(), e.section) {
                return Err(R5Error::Corrupt(format!(
                    "section {:?} out of bounds",
                    e.kind
                )));
            }
            if e.crc32_u32 != 0 {
                let start = e.section.off as usize;
                let end = start + e.section.len as usize;
                if end > data.len() {
                    return Err(R5Error::Corrupt("section crc OOB".into()));
                }
                let got = crc32_ieee(&data[start..end]);
                if got != e.crc32_u32 {
                    return Err(R5Error::Corrupt("section CRC mismatch".into()));
                }
            }
        }
        // Resolve sections
        let need = |k: SectionKind| -> Result<Section> {
            parse_toc(data, &header)
                .and_then(|t| t.into_iter().find(|e| e.kind == k).map(|e| e.section))
                .ok_or(R5Error::Invalid("missing required section"))
        };
        let id_sec = need(SectionKind::IdDict)?;
        let gn_sec = need(SectionKind::GNameDict)?;
        let term_sec = need(SectionKind::TermDict)?;
        let gdir = need(SectionKind::GDir)?;
        let idx_id2gid = need(SectionKind::IdxId2Gid)?;
        let idx_gname2gid = need(SectionKind::IdxGName2Gid)?;
        let idx_pair2gid = need(SectionKind::IdxPair2Gid)?;
        let triple_blocks = need(SectionKind::TripleBlocks)?;
        // Footer/global CRC if present
        if let Some((footer_crc, magic)) = parse_footer(data) {
            if &magic != b"R5TU_ENDMARK" {
                return Err(R5Error::Corrupt("bad footer magic".into()));
            }
            let got = crc32_ieee(&data[..data.len() - 16]);
            if got != footer_crc {
                return Err(R5Error::Corrupt("global CRC mismatch".into()));
            }
        }
        let id_dict = Dict::parse(data, id_sec)?;
        let gname_dict = Dict::parse(data, gn_sec)?;
        let term_dict = TermDict::parse(data, term_sec)?;
        Ok(Self {
            backing: Backing::Mmap(mmap),
            header,
            toc,
            id_dict,
            gname_dict,
            term_dict,
            gdir,
            idx_id2gid,
            idx_gname2gid,
            idx_pair2gid,
            triple_blocks,
        })
    }

    /// Returns the parsed file header.
    pub fn header(&self) -> &Header {
        &self.header
    }
    /// Returns the parsed table of contents (TOC).
    pub fn toc(&self) -> &[TocEntry] {
        &self.toc
    }

    /// Finds a section by kind and returns its byte span, if present.
    pub fn section(&self, kind: SectionKind) -> Option<Section> {
        self.toc.iter().find(|e| e.kind == kind).map(|e| e.section)
    }

    // API placeholders per ARCH.md §4.1
    /// Enumerate graph groups with a matching dataset id string.
    pub fn enumerate_by_id(&self, id: &str) -> Result<Vec<GraphRef>> {
        let Some(id_id) = self.id_dict.find_id(self.bytes(), id) else {
            return Ok(Vec::new());
        };
        self.postings_to_graphrefs(self.idx_id2gid, id_id as usize)
    }
    /// Enumerate graph groups with a matching graph name string.
    pub fn enumerate_by_graphname(&self, gname: &str) -> Result<Vec<GraphRef>> {
        let Some(gn_id) = self.gname_dict.find_id(self.bytes(), gname) else {
            return Ok(Vec::new());
        };
        self.postings_to_graphrefs(self.idx_gname2gid, gn_id as usize)
    }
    /// Resolve a (id, graphname) pair to a single group, if it exists.
    pub fn resolve_gid(&self, id: &str, gname: &str) -> Result<Option<GraphRef>> {
        let id_id = match self.id_dict.find_id(self.bytes(), id) {
            Some(v) => v,
            None => return Ok(None),
        };
        let gn_id = match self.gname_dict.find_id(self.bytes(), gname) {
            Some(v) => v,
            None => return Ok(None),
        };
        if let Some(gid) = self.pair_lookup(self.idx_pair2gid, id_id, gn_id)? {
            let gr = self.graphref_for_gid(gid)?;
            return Ok(Some(gr));
        }
        Ok(None)
    }
    /// Iterate over triples (S, P, O) as term ids for the given `gid`.
    ///
    /// Convert term ids to strings with [`Self::term_to_string`].
    pub fn triples_ids(&self, gid: u64) -> Result<TripleIter> {
        self.decode_triple_block(gid)
    }
    /// Resolve a term id to a displayable string (IRI, bnode, or literal).
    pub fn term_to_string(&self, term_id: u64) -> Result<String> {
        self.term_dict.term_to_string(self.bytes(), term_id)
    }

    /// Internal helper: convert a term id into the writer's [`crate::writer::Term`].
    ///
    /// Exposed as `pub(crate)` for modules that need to reconstruct quads
    /// faithfully (e.g., update routines).
    pub(crate) fn term_as_writer_term(&self, term_id: u64) -> Result<crate::writer::Term> {
        let parts = self.term_dict.term_parts(self.bytes(), term_id)?;
        let t = match parts {
            TermParts::Iri(s) => crate::writer::Term::Iri(s),
            TermParts::BNode(b) => crate::writer::Term::BNode(b),
            TermParts::Literal { lex, dt, lang } => crate::writer::Term::Literal { lex, dt, lang },
        };
        Ok(t)
    }

    #[cfg(feature = "oxigraph")]
    pub fn to_oxigraph_graph(&self, gid: u64) -> Result<oxigraph::model::Graph> {
        use oxigraph::model::{BlankNode, Graph, Literal, NamedNode, NamedOrBlankNode, Triple};
        let mut g = Graph::new();
        for (s_id, p_id, o_id) in self.triples_ids(gid)? {
            let s_parts = self.term_dict.term_parts(self.bytes(), s_id)?;
            let p_parts = self.term_dict.term_parts(self.bytes(), p_id)?;
            let o_parts = self.term_dict.term_parts(self.bytes(), o_id)?;
            let s_nb: NamedOrBlankNode = match s_parts {
                TermParts::Iri(s) => NamedNode::new(s)
                    .map_err(|_| R5Error::Invalid("invalid subject IRI"))?
                    .into(),
                TermParts::BNode(label) => {
                    let lbl = label.strip_prefix("_:").unwrap_or(&label).to_string();
                    BlankNode::new(lbl)
                        .map_err(|_| R5Error::Invalid("invalid blank node"))?
                        .into()
                }
                TermParts::Literal { .. } => return Err(R5Error::Invalid("literal subject")),
            };
            let p_nn = match p_parts {
                TermParts::Iri(p) => {
                    NamedNode::new(p).map_err(|_| R5Error::Invalid("invalid predicate IRI"))?
                }
                _ => return Err(R5Error::Invalid("non-IRI predicate")),
            };
            let o_term: oxigraph::model::Term = match o_parts {
                TermParts::Iri(o) => NamedNode::new(o)
                    .map_err(|_| R5Error::Invalid("invalid object IRI"))?
                    .into(),
                TermParts::BNode(label) => {
                    let lbl = label.strip_prefix("_:").unwrap_or(&label).to_string();
                    BlankNode::new(lbl)
                        .map_err(|_| R5Error::Invalid("invalid blank node"))?
                        .into()
                }
                TermParts::Literal { lex, dt, lang } => {
                    if let Some(dt) = dt {
                        let nn = NamedNode::new(dt)
                            .map_err(|_| R5Error::Invalid("invalid datatype IRI"))?;
                        Literal::new_typed_literal(lex, nn).into()
                    } else if let Some(lang) = lang {
                        Literal::new_language_tagged_literal(lex, lang)
                            .map_err(|_| R5Error::Invalid("invalid lang tag"))?
                            .into()
                    } else {
                        Literal::new_simple_literal(lex).into()
                    }
                }
            };
            g.insert(&Triple::new(s_nb, p_nn, o_term));
        }
        Ok(g)
    }

    #[cfg(feature = "oxigraph")]
    pub fn oxigraph_triples<'a>(&'a self, gid: u64) -> Result<OxTripleIter<'a>> {
        let inner = self.triples_ids(gid)?;
        Ok(OxTripleIter { file: self, inner })
    }

    // Enumerate all graphs across all graphnames.
    pub fn enumerate_all(&self) -> Result<Vec<GraphRef>> {
        let (n_rows, _) = self.gdir_header()?;
        let mut out = Vec::with_capacity(n_rows as usize);
        for gid in 0..n_rows {
            out.push(self.graphref_for_gid(gid)?);
        }
        Ok(out)
    }
}

// ---------------- Dicts (ID/GNAME) ----------------
#[derive(Debug, Clone, Copy)]
struct Dict {
    #[allow(dead_code)]
    sec: Section,
    n: u32,
    blob: Section,
    offs: Section,
    idx: Option<Section>,
}

impl Dict {
    fn parse(data: &[u8], sec: Section) -> Result<Self> {
        if !section_in_bounds(data.len(), sec) {
            return Err(R5Error::Corrupt("dict section OOB".into()));
        }
        let base = sec.off as usize;
        if base + 52 > data.len() {
            return Err(R5Error::Corrupt("short dict header".into()));
        }
        let n = u32::from_le_bytes([data[base], data[base + 1], data[base + 2], data[base + 3]]);
        let read_u64 = |o: usize| -> u64 {
            u64::from_le_bytes([
                data[o],
                data[o + 1],
                data[o + 2],
                data[o + 3],
                data[o + 4],
                data[o + 5],
                data[o + 6],
                data[o + 7],
            ])
        };
        let blob_off = read_u64(base + 4);
        let blob_len = read_u64(base + 12);
        let offs_off = read_u64(base + 20);
        let offs_len = read_u64(base + 28);
        let idx_off = read_u64(base + 36);
        let idx_len = read_u64(base + 44);

        let blob = Section {
            off: blob_off,
            len: blob_len,
        };
        let offs = Section {
            off: offs_off,
            len: offs_len,
        };
        let idx = if idx_off != 0 {
            Some(Section {
                off: idx_off,
                len: idx_len,
            })
        } else {
            None
        };
        if !section_in_bounds(data.len(), blob) || !section_in_bounds(data.len(), offs) {
            return Err(R5Error::Corrupt("dict blob/offs OOB".into()));
        }
        if let Some(s) = idx
            && !section_in_bounds(data.len(), s)
        {
            return Err(R5Error::Corrupt("dict index OOB".into()));
        }
        Ok(Dict {
            sec,
            n,
            blob,
            offs,
            idx,
        })
    }

    fn get<'a>(&self, data: &'a [u8], id: u32) -> Option<&'a str> {
        if id >= self.n {
            return None;
        }
        let o_base = self.offs.off as usize;
        let s = u32::from_le_bytes(
            data[o_base + id as usize * 4..o_base + id as usize * 4 + 4]
                .try_into()
                .ok()?,
        ) as usize;
        let e = u32::from_le_bytes(
            data[o_base + (id as usize + 1) * 4..o_base + (id as usize + 1) * 4 + 4]
                .try_into()
                .ok()?,
        ) as usize;
        let b_base = self.blob.off as usize;
        std::str::from_utf8(&data[b_base + s..b_base + e]).ok()
    }

    fn find_id(&self, data: &[u8], s: &str) -> Option<u32> {
        if let Some(idx) = self.idx {
            let ib = idx.off as usize;
            let n = self.n as usize;
            let mut key16 = [0u8; 16];
            for (i, b) in s
                .to_ascii_lowercase()
                .as_bytes()
                .iter()
                .take(16)
                .enumerate()
            {
                key16[i] = *b;
            }
            let mut lo = 0usize;
            let mut hi = n;
            while lo < hi {
                let mid = (lo + hi) / 2;
                let off = ib + mid * 24;
                let k = &data[off..off + 16];
                use std::cmp::Ordering::*;
                match k.cmp(&key16) {
                    Less => lo = mid + 1,
                    Greater => hi = mid,
                    Equal => {
                        // scan neighbors with identical key16
                        let mut m = mid;
                        while m > 0 && &data[ib + (m - 1) * 24..ib + (m - 1) * 24 + 16] == k {
                            m -= 1;
                        }
                        while m < n && &data[ib + m * 24..ib + m * 24 + 16] == k {
                            let id = u32::from_le_bytes(
                                data[ib + m * 24 + 16..ib + m * 24 + 20].try_into().ok()?,
                            );
                            if let Some(ss) = self.get(data, id)
                                && ss == s
                            {
                                return Some(id);
                            }
                            m += 1;
                        }
                        return None;
                    }
                }
            }
            None
        } else {
            // fallback linear search
            for i in 0..self.n {
                if let Some(ss) = self.get(data, i)
                    && ss == s
                {
                    return Some(i);
                }
            }
            None
        }
    }
}

// ---------------- Term Dict ----------------
#[derive(Debug, Clone, Copy)]
struct TermDict {
    n_terms: u64,
    kinds_off: u64,
    data_off: u64,
    offs_off: u64,
}

impl TermDict {
    fn parse(data: &[u8], sec: Section) -> Result<Self> {
        if !section_in_bounds(data.len(), sec) {
            return Err(R5Error::Corrupt("term dict OOB".into()));
        }
        let base = sec.off as usize;
        if base + 1 + 8 * 4 > data.len() {
            return Err(R5Error::Corrupt("short term dict header".into()));
        }
        let _width = data[base]; // reserved
        let n_terms = u64::from_le_bytes(data[base + 1..base + 9].try_into().unwrap());
        let kinds_off = u64::from_le_bytes(data[base + 9..base + 17].try_into().unwrap());
        let data_off = u64::from_le_bytes(data[base + 17..base + 25].try_into().unwrap());
        let offs_off = u64::from_le_bytes(data[base + 25..base + 33].try_into().unwrap());
        Ok(TermDict {
            n_terms,
            kinds_off,
            data_off,
            offs_off,
        })
    }

    fn term_to_string(&self, data: &[u8], term_id: u64) -> Result<String> {
        if term_id >= self.n_terms {
            return Err(R5Error::Invalid("term id out of range"));
        }
        let kinds_off = self.kinds_off as usize;
        let data_off = self.data_off as usize;
        let offs_off = self.offs_off as usize;
        // offs is u64 * (n+1)
        let s = u64::from_le_bytes(
            data[offs_off + term_id as usize * 8..offs_off + term_id as usize * 8 + 8]
                .try_into()
                .unwrap(),
        ) as usize;
        let e = u64::from_le_bytes(
            data[offs_off + (term_id as usize + 1) * 8..offs_off + (term_id as usize + 1) * 8 + 8]
                .try_into()
                .unwrap(),
        ) as usize;
        let payload = &data[data_off + s..data_off + e];
        match data[kinds_off + term_id as usize] {
            0 | 1 => std::str::from_utf8(payload)
                .map(String::from)
                .map_err(|_| R5Error::Corrupt("utf8".into())),
            2 => {
                let (lex_len, mut off) =
                    read_uvarint(payload, 0).ok_or_else(|| R5Error::Corrupt("lit lex".into()))?;
                let lex = std::str::from_utf8(&payload[off..off + lex_len as usize])
                    .map_err(|_| R5Error::Corrupt("utf8".into()))?;
                off += lex_len as usize;
                if off >= payload.len() {
                    return Err(R5Error::Corrupt("lit bounds".into()));
                }
                let has_dt = payload[off];
                off += 1;
                let dt = if has_dt == 1 {
                    let (l, o2) = read_uvarint(payload, off)
                        .ok_or_else(|| R5Error::Corrupt("dt len".into()))?;
                    let s = std::str::from_utf8(&payload[o2..o2 + l as usize])
                        .map_err(|_| R5Error::Corrupt("utf8".into()))?;
                    off = o2 + l as usize;
                    Some(s.to_string())
                } else {
                    None
                };
                if off >= payload.len() {
                    return Err(R5Error::Corrupt("lit bounds2".into()));
                }
                let has_lang = payload[off];
                off += 1;
                let lang = if has_lang == 1 {
                    let (l, o2) = read_uvarint(payload, off)
                        .ok_or_else(|| R5Error::Corrupt("lang len".into()))?;
                    let s = std::str::from_utf8(&payload[o2..o2 + l as usize])
                        .map_err(|_| R5Error::Corrupt("utf8".into()))?;
                    Some(s.to_string())
                } else {
                    None
                };
                Ok(match (dt, lang) {
                    (Some(dt), _) => format!("\"{}\"^^<{}>", lex, dt),
                    (None, Some(lang)) => format!("\"{}\"@{}", lex, lang),
                    _ => format!("\"{}\"", lex),
                })
            }
            _ => Err(R5Error::Corrupt("unknown term kind".into())),
        }
    }

    // Exposed for internal crate use (e.g., update module) to faithfully
    // reconstruct writer terms from an existing file without going through
    // a third-party representation.
    pub(crate) fn term_parts(&self, data: &[u8], term_id: u64) -> Result<TermParts> {
        if term_id >= self.n_terms {
            return Err(R5Error::Invalid("term id out of range"));
        }
        let kinds_off = self.kinds_off as usize;
        let data_off = self.data_off as usize;
        let offs_off = self.offs_off as usize;
        let s = u64::from_le_bytes(
            data[offs_off + term_id as usize * 8..offs_off + term_id as usize * 8 + 8]
                .try_into()
                .unwrap(),
        ) as usize;
        let e = u64::from_le_bytes(
            data[offs_off + (term_id as usize + 1) * 8..offs_off + (term_id as usize + 1) * 8 + 8]
                .try_into()
                .unwrap(),
        ) as usize;
        let payload = &data[data_off + s..data_off + e];
        Ok(match data[kinds_off + term_id as usize] {
            0 => TermParts::Iri(
                std::str::from_utf8(payload)
                    .map_err(|_| R5Error::Corrupt("utf8".into()))?
                    .to_string(),
            ),
            1 => TermParts::BNode(
                std::str::from_utf8(payload)
                    .map_err(|_| R5Error::Corrupt("utf8".into()))?
                    .to_string(),
            ),
            2 => {
                let (lex_len, mut off) =
                    read_uvarint(payload, 0).ok_or_else(|| R5Error::Corrupt("lit lex".into()))?;
                let lex = std::str::from_utf8(&payload[off..off + lex_len as usize])
                    .map_err(|_| R5Error::Corrupt("utf8".into()))?
                    .to_string();
                off += lex_len as usize;
                if off >= payload.len() {
                    return Err(R5Error::Corrupt("lit bounds".into()));
                }
                let has_dt = payload[off];
                off += 1;
                let dt = if has_dt == 1 {
                    let (l, o2) = read_uvarint(payload, off)
                        .ok_or_else(|| R5Error::Corrupt("dt len".into()))?;
                    off = o2;
                    let s = std::str::from_utf8(&payload[off..off + l as usize])
                        .map_err(|_| R5Error::Corrupt("utf8".into()))?
                        .to_string();
                    off += l as usize;
                    Some(s)
                } else {
                    None
                };
                if off >= payload.len() {
                    return Err(R5Error::Corrupt("lit bounds2".into()));
                }
                let has_lang = payload[off];
                off += 1;
                let lang = if has_lang == 1 {
                    let (l, o2) = read_uvarint(payload, off)
                        .ok_or_else(|| R5Error::Corrupt("lang len".into()))?;
                    off = o2;
                    let s = std::str::from_utf8(&payload[off..off + l as usize])
                        .map_err(|_| R5Error::Corrupt("utf8".into()))?
                        .to_string();
                    Some(s)
                } else {
                    None
                };
                TermParts::Literal { lex, dt, lang }
            }
            _ => return Err(R5Error::Corrupt("unknown term kind".into())),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum TermParts {
    Iri(String),
    BNode(String),
    Literal {
        lex: String,
        dt: Option<String>,
        lang: Option<String>,
    },
}

#[cfg(feature = "oxigraph")]
pub struct OxTripleIter<'a> {
    file: &'a R5tuFile,
    inner: TripleIter,
}

#[cfg(feature = "oxigraph")]
impl<'a> Iterator for OxTripleIter<'a> {
    type Item = Result<oxigraph::model::Triple>;
    fn next(&mut self) -> Option<Self::Item> {
        use oxigraph::model::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Triple};
        let (s_id, p_id, o_id) = self.inner.next()?;
        let bytes = self.file.bytes();
        let s_parts = match self.file.term_dict.term_parts(bytes, s_id) {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        };
        let p_parts = match self.file.term_dict.term_parts(bytes, p_id) {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        };
        let o_parts = match self.file.term_dict.term_parts(bytes, o_id) {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        };
        let s_nb: NamedOrBlankNode = match s_parts {
            TermParts::Iri(s) => match NamedNode::new(s) {
                Ok(n) => n.into(),
                Err(_) => return Some(Err(R5Error::Invalid("invalid subject IRI"))),
            },
            TermParts::BNode(label) => {
                let lbl = label.strip_prefix("_:").unwrap_or(&label).to_string();
                match BlankNode::new(lbl) {
                    Ok(b) => b.into(),
                    Err(_) => return Some(Err(R5Error::Invalid("invalid blank node"))),
                }
            }
            TermParts::Literal { .. } => return Some(Err(R5Error::Invalid("literal subject"))),
        };
        let p_nn = match p_parts {
            TermParts::Iri(p) => match NamedNode::new(p) {
                Ok(n) => n,
                Err(_) => return Some(Err(R5Error::Invalid("invalid predicate IRI"))),
            },
            _ => return Some(Err(R5Error::Invalid("non-IRI predicate"))),
        };
        let o_term: oxigraph::model::Term = match o_parts {
            TermParts::Iri(o) => match NamedNode::new(o) {
                Ok(n) => n.into(),
                Err(_) => return Some(Err(R5Error::Invalid("invalid object IRI"))),
            },
            TermParts::BNode(label) => {
                let lbl = label.strip_prefix("_:").unwrap_or(&label).to_string();
                match BlankNode::new(lbl) {
                    Ok(b) => b.into(),
                    Err(_) => return Some(Err(R5Error::Invalid("invalid blank node"))),
                }
            }
            TermParts::Literal { lex, dt, lang } => {
                if let Some(dt) = dt {
                    let nn = match NamedNode::new(dt) {
                        Ok(n) => n,
                        Err(_) => return Some(Err(R5Error::Invalid("invalid datatype IRI"))),
                    };
                    Literal::new_typed_literal(lex, nn).into()
                } else if let Some(lang) = lang {
                    match Literal::new_language_tagged_literal(lex, lang) {
                        Ok(l) => l.into(),
                        Err(_) => return Some(Err(R5Error::Invalid("invalid lang tag"))),
                    }
                } else {
                    Literal::new_simple_literal(lex).into()
                }
            }
        };
        Some(Ok(Triple::new(s_nb, p_nn, o_term)))
    }
}

// ---------------- GDIR and GraphRefs ----------------
#[derive(Debug, Clone, Copy)]
struct GDirRow {
    id_id: u32,
    gn_id: u32,
    triples_off: u64,
    triples_len: u64,
    n_triples: u64,
    #[allow(dead_code)]
    n_s: u32,
    #[allow(dead_code)]
    n_p: u32,
    #[allow(dead_code)]
    n_o: u32,
}

impl R5tuFile {
    fn gdir_header(&self) -> Result<(u64, usize)> {
        let bytes = self.bytes();
        let base = self.gdir.off as usize;
        if base + 16 > bytes.len() {
            return Err(R5Error::Corrupt("gdir header OOB".into()));
        }
        let n_rows = u64::from_le_bytes(bytes[base..base + 8].try_into().unwrap());
        let row_size = u32::from_le_bytes(bytes[base + 8..base + 12].try_into().unwrap()) as usize;
        Ok((n_rows, row_size))
    }

    fn gdir_row(&self, gid: u64) -> Result<GDirRow> {
        let (n_rows, row_size) = self.gdir_header()?;
        if gid >= n_rows {
            return Err(R5Error::Invalid("gid out of range"));
        }
        let bytes = self.bytes();
        let off = self.gdir.off as usize + 16 + gid as usize * row_size;
        if off + row_size > bytes.len() {
            return Err(R5Error::Corrupt("gdir row OOB".into()));
        }
        let b = &bytes[off..off + row_size];
        Ok(GDirRow {
            id_id: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            gn_id: u32::from_le_bytes(b[4..8].try_into().unwrap()),
            triples_off: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            triples_len: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            n_triples: u64::from_le_bytes(b[24..32].try_into().unwrap()),
            n_s: u32::from_le_bytes(b[32..36].try_into().unwrap()),
            n_p: u32::from_le_bytes(b[36..40].try_into().unwrap()),
            n_o: u32::from_le_bytes(b[40..44].try_into().unwrap()),
        })
    }

    fn graphref_for_gid(&self, gid: u64) -> Result<GraphRef> {
        let row = self.gdir_row(gid)?;
        let bytes = self.bytes();
        let id = self
            .id_dict
            .get(bytes, row.id_id)
            .ok_or(R5Error::Corrupt("id str OOB".into()))?
            .to_string();
        let graphname = self
            .gname_dict
            .get(bytes, row.gn_id)
            .ok_or(R5Error::Corrupt("gname str OOB".into()))?
            .to_string();
        Ok(GraphRef {
            gid,
            id,
            graphname,
            n_triples: row.n_triples,
        })
    }
}

// ---------------- Postings & Pair index ----------------
impl R5tuFile {
    fn postings_to_graphrefs(&self, sec: Section, key_ordinal: usize) -> Result<Vec<GraphRef>> {
        let gids = self.decode_posting_list(sec, key_ordinal)?;
        let mut out = Vec::with_capacity(gids.len());
        for gid in gids {
            out.push(self.graphref_for_gid(gid)?);
        }
        Ok(out)
    }

    fn decode_posting_list(&self, sec: Section, key_ordinal: usize) -> Result<Vec<u64>> {
        let data_all = self.bytes();
        let b = &data_all[sec.off as usize..(sec.off + sec.len) as usize];
        if b.len() < 24 {
            return Err(R5Error::Corrupt("postings header short".into()));
        }
        let n_keys = u64::from_le_bytes(b[0..8].try_into().unwrap()) as usize;
        if key_ordinal >= n_keys {
            return Ok(vec![]);
        }
        let offs_off = u64::from_le_bytes(b[8..16].try_into().unwrap()) as usize;
        let blob_off = u64::from_le_bytes(b[16..24].try_into().unwrap()) as usize;
        let data = self.bytes();
        if offs_off + (n_keys + 1) * 8 > data.len() {
            return Err(R5Error::Corrupt("postings offs OOB".into()));
        }
        let s = u64::from_le_bytes(
            data[offs_off + key_ordinal * 8..offs_off + key_ordinal * 8 + 8]
                .try_into()
                .unwrap(),
        ) as usize;
        let e = u64::from_le_bytes(
            data[offs_off + (key_ordinal + 1) * 8..offs_off + (key_ordinal + 1) * 8 + 8]
                .try_into()
                .unwrap(),
        ) as usize;
        if blob_off + e > data.len() || blob_off + s > data.len() || s > e {
            return Err(R5Error::Corrupt("postings blob OOB".into()));
        }
        let mut off = blob_off + s;
        let end = blob_off + e;
        let (n, o1) =
            read_uvarint(data, off).ok_or_else(|| R5Error::Corrupt("postings n".into()))?;
        off = o1;
        if n == 0 {
            return Ok(vec![]);
        }
        let (first, o_after_first) =
            read_uvarint(data, off).ok_or_else(|| R5Error::Corrupt("postings first".into()))?;
        off = o_after_first;
        let mut out = Vec::with_capacity(n as usize);
        out.push(first);
        let mut cur = first;
        for _ in 1..n {
            if off >= end {
                return Err(R5Error::Corrupt("postings truncated".into()));
            }
            let (d, o2) =
                read_uvarint(data, off).ok_or_else(|| R5Error::Corrupt("postings delta".into()))?;
            off = o2;
            cur = cur
                .checked_add(d)
                .ok_or_else(|| R5Error::Corrupt("postings overflow".into()))?;
            out.push(cur);
        }
        Ok(out)
    }

    fn pair_lookup(&self, sec: Section, id_id: u32, gn_id: u32) -> Result<Option<u64>> {
        let data = self.bytes();
        let b = &data[sec.off as usize..(sec.off + sec.len) as usize];
        if b.len() < 16 {
            return Err(R5Error::Corrupt("pair idx short".into()));
        }
        let n_pairs = u64::from_le_bytes(b[0..8].try_into().unwrap()) as usize;
        let pairs_off = u64::from_le_bytes(b[8..16].try_into().unwrap()) as usize;
        let entry_size = 16usize;
        if pairs_off + n_pairs * entry_size > data.len() {
            return Err(R5Error::Corrupt("pairs OOB".into()));
        }
        let mut lo = 0usize;
        let mut hi = n_pairs;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let off = pairs_off + mid * entry_size;
            let mid_id = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
            let mid_gn = u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap());
            use std::cmp::Ordering::*;
            match (mid_id, mid_gn).cmp(&(id_id, gn_id)) {
                Less => lo = mid + 1,
                Greater => hi = mid,
                Equal => {
                    let gid = u64::from_le_bytes(data[off + 8..off + 16].try_into().unwrap());
                    return Ok(Some(gid));
                }
            }
        }
        Ok(None)
    }
}

// ---------------- Utilities ----------------
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

// ---------------- Triple blocks ----------------
#[derive(Debug)]
pub struct TripleIter {
    s_vals: Vec<u64>,
    s_heads: Vec<u64>,
    p_vals: Vec<u64>,
    p_heads: Vec<u64>,
    o_vals: Vec<u64>,
    si: usize,
    pi: usize,
    oi: usize,
}

impl Iterator for TripleIter {
    type Item = (u64, u64, u64);
    fn next(&mut self) -> Option<Self::Item> {
        if self.oi >= self.o_vals.len() {
            return None;
        }
        while self.pi + 1 < self.p_heads.len() && self.p_heads[self.pi + 1] <= self.oi as u64 {
            self.pi += 1;
        }
        while self.si + 1 < self.s_heads.len() && self.s_heads[self.si + 1] <= self.pi as u64 {
            self.si += 1;
        }
        let s = self.s_vals[self.si];
        let p = self.p_vals[self.pi];
        let o = self.o_vals[self.oi];
        self.oi += 1;
        Some((s, p, o))
    }
}

impl R5tuFile {
    fn decode_triple_block(&self, gid: u64) -> Result<TripleIter> {
        let row = self.gdir_row(gid)?;
        let data = self.bytes();
        let base = row.triples_off as usize;
        let end = base
            .checked_add(row.triples_len as usize)
            .ok_or_else(|| R5Error::Corrupt("block bounds".into()))?;
        if end > data.len() {
            return Err(R5Error::Corrupt("block OOB".into()));
        }
        if base + 1 + 4 > end {
            return Err(R5Error::Corrupt("block header short".into()));
        }
        let enc = data[base];
        let raw_len = u32::from_le_bytes(data[base + 1..base + 5].try_into().unwrap()) as usize;
        let payload_start = base + 5;
        match enc {
            0 => {
                if payload_start + raw_len > end {
                    return Err(R5Error::Corrupt("raw len OOB".into()));
                }
                let raw = &data[payload_start..payload_start + raw_len];
                self.decode_raw_payload(raw)
            }
            1 => {
                #[cfg(feature = "zstd")]
                {
                    if payload_start + raw_len > end {
                        return Err(R5Error::Corrupt("zstd len OOB".into()));
                    }
                    let frame = &data[payload_start..payload_start + raw_len];
                    let raw = zstd::decode_all(std::io::Cursor::new(frame))
                        .map_err(|_| R5Error::Corrupt("zstd decode".into()))?;
                    self.decode_raw_payload(&raw)
                }
                #[cfg(not(feature = "zstd"))]
                {
                    Err(R5Error::Invalid("zstd feature not enabled"))
                }
            }
            _ => Err(R5Error::Corrupt("unknown block encoding".into())),
        }
    }

    fn decode_raw_payload(&self, raw: &[u8]) -> Result<TripleIter> {
        let mut off = 0usize;
        let (n_s, o1) = read_uvarint(raw, off).ok_or_else(|| R5Error::Corrupt("nS".into()))?;
        off = o1;
        let (n_p, o2) = read_uvarint(raw, off).ok_or_else(|| R5Error::Corrupt("nP".into()))?;
        off = o2;
        let (n_t, o3) = read_uvarint(raw, off).ok_or_else(|| R5Error::Corrupt("nT".into()))?;
        off = o3;
        let n_s = n_s as usize;
        let n_p = n_p as usize;
        let n_t = n_t as usize;
        // S_vals (delta-coded ascending)
        let mut s_vals = Vec::with_capacity(n_s);
        if n_s > 0 {
            let (first, o) =
                read_uvarint(raw, off).ok_or_else(|| R5Error::Corrupt("S first".into()))?;
            off = o;
            s_vals.push(first);
            for _ in 1..n_s {
                let (d, o2) =
                    read_uvarint(raw, off).ok_or_else(|| R5Error::Corrupt("S delta".into()))?;
                off = o2;
                let prev = *s_vals.last().unwrap();
                s_vals.push(
                    prev.checked_add(d)
                        .ok_or_else(|| R5Error::Corrupt("S overflow".into()))?,
                );
            }
        }
        // S_heads (prefix sums into P)
        let mut s_heads = Vec::with_capacity(n_s + 1);
        for _ in 0..(n_s + 1) {
            let (v, o) =
                read_uvarint(raw, off).ok_or_else(|| R5Error::Corrupt("S_heads".into()))?;
            off = o;
            s_heads.push(v);
        }
        if *s_heads.last().unwrap_or(&0) as usize != n_p {
            return Err(R5Error::Corrupt("S_heads last != nP".into()));
        }

        // P_vals (delta-coded per S-run)
        let mut p_vals = vec![0u64; n_p];
        for s in 0..n_s {
            let start = s_heads[s] as usize;
            let end = s_heads[s + 1] as usize;
            if start > end || end > n_p {
                return Err(R5Error::Corrupt("P run OOB".into()));
            }
            if start == end {
                continue;
            }
            // first absolute in run
            let (first, o) =
                read_uvarint(raw, off).ok_or_else(|| R5Error::Corrupt("P first".into()))?;
            off = o;
            p_vals[start] = first;
            let mut cur = first;
            for v in p_vals[start + 1..end].iter_mut() {
                let (d, o2) =
                    read_uvarint(raw, off).ok_or_else(|| R5Error::Corrupt("P delta".into()))?;
                off = o2;
                cur = cur
                    .checked_add(d)
                    .ok_or_else(|| R5Error::Corrupt("P overflow".into()))?;
                *v = cur;
            }
        }
        // P_heads (prefix sums into O)
        let mut p_heads = Vec::with_capacity(n_p + 1);
        for _ in 0..(n_p + 1) {
            let (v, o) =
                read_uvarint(raw, off).ok_or_else(|| R5Error::Corrupt("P_heads".into()))?;
            off = o;
            p_heads.push(v);
        }
        if *p_heads.last().unwrap_or(&0) as usize != n_t {
            return Err(R5Error::Corrupt("P_heads last != nT".into()));
        }

        // O_vals (delta-coded per (S,P)-run)
        let mut o_vals = vec![0u64; n_t];
        for p in 0..n_p {
            let start = p_heads[p] as usize;
            let end = p_heads[p + 1] as usize;
            if start > end || end > n_t {
                return Err(R5Error::Corrupt("O run OOB".into()));
            }
            if start == end {
                continue;
            }
            let (first, o) =
                read_uvarint(raw, off).ok_or_else(|| R5Error::Corrupt("O first".into()))?;
            off = o;
            o_vals[start] = first;
            let mut cur = first;
            for v in o_vals[start + 1..end].iter_mut() {
                let (d, o2) =
                    read_uvarint(raw, off).ok_or_else(|| R5Error::Corrupt("O delta".into()))?;
                off = o2;
                cur = cur
                    .checked_add(d)
                    .ok_or_else(|| R5Error::Corrupt("O overflow".into()))?;
                *v = cur;
            }
        }

        Ok(TripleIter {
            s_vals,
            s_heads,
            p_vals,
            p_heads,
            o_vals,
            si: 0,
            pi: 0,
            oi: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn push_uvarint(v: u64, out: &mut Vec<u8>) {
        let mut x = v;
        loop {
            let mut b = (x & 0x7f) as u8;
            x >>= 7;
            if x != 0 {
                b |= 0x80;
            }
            out.push(b);
            if x == 0 {
                break;
            }
        }
    }

    // minimal smoke: invalid header rejected
    #[test]
    fn rejects_short_or_bad_magic() {
        let mut path = std::env::temp_dir();
        path.push("bad.r5tu");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(b"NOPE").unwrap();
        let err = R5tuFile::open(&path).unwrap_err();
        match err {
            R5Error::Invalid(_) => {}
            _ => panic!("expected Invalid"),
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn uvarint_roundtrip_and_bounds() {
        let mut buf = Vec::new();
        for &n in &[
            0,
            1,
            127,
            128,
            255,
            16384,
            u32::MAX as u64,
            u64::from(u32::MAX) + 12345,
        ] {
            buf.clear();
            push_uvarint(n, &mut buf);
            let (v, off) = read_uvarint(&buf, 0).unwrap();
            assert_eq!(v, n);
            assert_eq!(off, buf.len());
        }
        assert!(read_uvarint(&[], 0).is_none());
    }

    #[test]
    fn dict_parse_and_lookup() {
        // Build a minimal dict section inline: entries ["A", "B"]
        let mut file = vec![0u8; 0];
        let sec_off = file.len();
        file.resize(file.len() + 52, 0); // dict header
        // payload: blob and offs
        let blob_off = file.len();
        file.extend_from_slice(b"AB");
        let offs_off = file.len();
        // offs [0,1,2]
        for n in [0u32, 1, 2] {
            file.extend_from_slice(&n.to_le_bytes());
        }
        // fill header
        let n = 2u32;
        file[sec_off..sec_off + 4].copy_from_slice(&n.to_le_bytes());
        let blob_off_u64 = (blob_off as u64).to_le_bytes();
        let blob_len_u64 = (2u64).to_le_bytes();
        let offs_off_u64 = (offs_off as u64).to_le_bytes();
        let offs_len_u64 = (12u64).to_le_bytes();
        file[sec_off + 4..sec_off + 12].copy_from_slice(&blob_off_u64);
        file[sec_off + 12..sec_off + 20].copy_from_slice(&blob_len_u64);
        file[sec_off + 20..sec_off + 28].copy_from_slice(&offs_off_u64);
        file[sec_off + 28..sec_off + 36].copy_from_slice(&offs_len_u64);
        // idx absent (zeros)

        let dict = Dict::parse(
            &file,
            Section {
                off: sec_off as u64,
                len: (file.len() - sec_off) as u64,
            },
        )
        .unwrap();
        assert_eq!(dict.get(&file, 0).unwrap(), "A");
        assert_eq!(dict.get(&file, 1).unwrap(), "B");
        assert_eq!(dict.find_id(&file, "A"), Some(0));
        assert_eq!(dict.find_id(&file, "B"), Some(1));
        assert_eq!(dict.find_id(&file, "Z"), None);
    }

    #[test]
    fn term_dict_decode() {
        // One IRI, one literal with dt, one literal with lang
        let mut file = vec![0u8; 0];
        let sec_off = file.len();
        file.resize(file.len() + 33, 0); // term dict header
        // payload regions
        // kinds: [0=IRI, 2=LITERAL, 2=LITERAL]
        let kinds_off = file.len();
        file.extend_from_slice(&[0u8, 2, 2]);
        // data blob
        let data_off = file.len();
        // IRI payload is raw UTF-8
        let iri_bytes = b"http://ex/s";
        file.extend_from_slice(iri_bytes);
        // Literal with dt: "42"^^<http://ex/i>
        let mut lit1 = Vec::new();
        push_uvarint(2, &mut lit1);
        lit1.extend_from_slice(b"42"); // lex
        lit1.push(1); // has_dt
        push_uvarint(11, &mut lit1);
        lit1.extend_from_slice(b"http://ex/i");
        lit1.push(0); // has_lang
        file.extend_from_slice(&lit1);
        // Literal with lang: "en"@en
        let mut lit2 = Vec::new();
        push_uvarint(2, &mut lit2);
        lit2.extend_from_slice(b"en");
        lit2.push(0); // no dt
        lit2.push(1); // has_lang
        push_uvarint(2, &mut lit2);
        lit2.extend_from_slice(b"en");
        file.extend_from_slice(&lit2);
        // offs: u64*(n+1) = 4 entries
        let offs_off = file.len();
        let mut cur = 0u64;
        let sizes = [iri_bytes.len() as u64, lit1.len() as u64, lit2.len() as u64];
        file.extend_from_slice(&cur.to_le_bytes());
        cur += sizes[0];
        file.extend_from_slice(&cur.to_le_bytes());
        cur += sizes[1];
        file.extend_from_slice(&cur.to_le_bytes());
        cur += sizes[2];
        file.extend_from_slice(&cur.to_le_bytes());
        // fill header
        file[sec_off] = 0; // width
        let n_terms = (3u64).to_le_bytes();
        file[sec_off + 1..sec_off + 9].copy_from_slice(&n_terms);
        file[sec_off + 9..sec_off + 17].copy_from_slice(&(kinds_off as u64).to_le_bytes());
        file[sec_off + 17..sec_off + 25].copy_from_slice(&(data_off as u64).to_le_bytes());
        file[sec_off + 25..sec_off + 33].copy_from_slice(&(offs_off as u64).to_le_bytes());

        let td = TermDict::parse(
            &file,
            Section {
                off: sec_off as u64,
                len: (file.len() - sec_off) as u64,
            },
        )
        .unwrap();
        assert_eq!(td.term_to_string(&file, 0).unwrap(), "http://ex/s");
        assert_eq!(
            td.term_to_string(&file, 1).unwrap(),
            "\"42\"^^<http://ex/i>"
        );
        assert_eq!(td.term_to_string(&file, 2).unwrap(), "\"en\"@en");
    }

    #[test]
    fn end_to_end_minimal_file() {
        // Build a minimal complete file per ARCH to exercise enumerate*, resolve_gid, and triples iterator.
        let mut f = vec![0u8; 32]; // header placeholder
        let mut toc_entries: Vec<(SectionKind, u64, u64)> = Vec::new();

        // Helper to register a section
        let mut add_sec = |kind: SectionKind, off: usize, len: usize| {
            toc_entries.push((kind, off as u64, len as u64));
        };

        // ID_DICT with ["A"]
        let id_sec_off = f.len();
        f.resize(f.len() + 52, 0);
        let id_blob_off = f.len();
        f.extend_from_slice(b"A");
        let id_offs_off = f.len();
        for n in [0u32, 1] {
            f.extend_from_slice(&n.to_le_bytes());
        }
        // fill header
        f[id_sec_off..id_sec_off + 4].copy_from_slice(&1u32.to_le_bytes());
        f[id_sec_off + 4..id_sec_off + 12].copy_from_slice(&(id_blob_off as u64).to_le_bytes());
        f[id_sec_off + 12..id_sec_off + 20].copy_from_slice(&(1u64).to_le_bytes());
        f[id_sec_off + 20..id_sec_off + 28].copy_from_slice(&(id_offs_off as u64).to_le_bytes());
        f[id_sec_off + 28..id_sec_off + 36].copy_from_slice(&(8u64).to_le_bytes());
        add_sec(SectionKind::IdDict, id_sec_off, f.len() - id_sec_off);

        // GNAME_DICT with ["g"]
        let gn_sec_off = f.len();
        f.resize(f.len() + 52, 0);
        let gn_blob_off = f.len();
        f.extend_from_slice(b"g");
        let gn_offs_off = f.len();
        for n in [0u32, 1] {
            f.extend_from_slice(&n.to_le_bytes());
        }
        f[gn_sec_off..gn_sec_off + 4].copy_from_slice(&1u32.to_le_bytes());
        f[gn_sec_off + 4..gn_sec_off + 12].copy_from_slice(&(gn_blob_off as u64).to_le_bytes());
        f[gn_sec_off + 12..gn_sec_off + 20].copy_from_slice(&(1u64).to_le_bytes());
        f[gn_sec_off + 20..gn_sec_off + 28].copy_from_slice(&(gn_offs_off as u64).to_le_bytes());
        f[gn_sec_off + 28..gn_sec_off + 36].copy_from_slice(&(8u64).to_le_bytes());
        add_sec(SectionKind::GNameDict, gn_sec_off, f.len() - gn_sec_off);

        // TERM_DICT empty but valid
        let td_sec_off = f.len();
        f.resize(f.len() + 33, 0);
        let kinds_off = f.len(); // empty kinds
        let data_off = f.len(); // empty data
        let offs_off = f.len();
        f.extend_from_slice(&0u64.to_le_bytes()); // single 0
        f[td_sec_off] = 0;
        f[td_sec_off + 1..td_sec_off + 9].copy_from_slice(&0u64.to_le_bytes());
        f[td_sec_off + 9..td_sec_off + 17].copy_from_slice(&(kinds_off as u64).to_le_bytes());
        f[td_sec_off + 17..td_sec_off + 25].copy_from_slice(&(data_off as u64).to_le_bytes());
        f[td_sec_off + 25..td_sec_off + 33].copy_from_slice(&(offs_off as u64).to_le_bytes());
        add_sec(SectionKind::TermDict, td_sec_off, f.len() - td_sec_off);

        // TRIPLE_BLOCKS with one RAW block for gid=0, triples: (1,2,3) and (1,4,5)
        let tb_sec_off = f.len();
        let block_off = f.len();
        let mut raw = Vec::new();
        // nS=1, nP=2, nT=2
        push_uvarint(1, &mut raw);
        push_uvarint(2, &mut raw);
        push_uvarint(2, &mut raw);
        // S_vals: [1]
        push_uvarint(1, &mut raw);
        // S_heads: [0,2]
        push_uvarint(0, &mut raw);
        push_uvarint(2, &mut raw);
        // P_vals for S run: [2,4]
        push_uvarint(2, &mut raw); // first absolute
        push_uvarint(2, &mut raw); // delta = 2 (4-2)
        // P_heads: [0,1,2]
        push_uvarint(0, &mut raw);
        push_uvarint(1, &mut raw);
        push_uvarint(2, &mut raw);
        // O_vals per P run: [3] then [5]
        push_uvarint(3, &mut raw);
        push_uvarint(5, &mut raw);
        // block header
        f.push(0u8); // enc = RAW
        let raw_len = raw.len() as u32;
        f.extend_from_slice(&raw_len.to_le_bytes());
        f.extend_from_slice(&raw);
        let block_len = f.len() - block_off;
        add_sec(SectionKind::TripleBlocks, tb_sec_off, f.len() - tb_sec_off);

        // GDIR with 1 row
        let gdir_sec_off = f.len();
        // header
        f.extend_from_slice(&1u64.to_le_bytes()); // n_rows
        f.extend_from_slice(&56u32.to_le_bytes()); // row_size
        f.extend_from_slice(&0u32.to_le_bytes()); // reserved
        // row 0
        f.extend_from_slice(&0u32.to_le_bytes()); // id_id
        f.extend_from_slice(&0u32.to_le_bytes()); // gn_id
        f.extend_from_slice(&(block_off as u64).to_le_bytes());
        f.extend_from_slice(&(block_len as u64).to_le_bytes());
        f.extend_from_slice(&2u64.to_le_bytes()); // n_triples
        f.extend_from_slice(&1u32.to_le_bytes()); // n_s
        f.extend_from_slice(&2u32.to_le_bytes()); // n_p
        f.extend_from_slice(&2u32.to_le_bytes()); // n_o
        add_sec(SectionKind::GDir, gdir_sec_off, f.len() - gdir_sec_off);

        // IDX_ID2GID with 1 key -> [0]
        let ididx_sec_off = f.len();
        let ididx_hdr_off = ididx_sec_off;
        f.resize(f.len() + 24, 0);
        let ididx_offs_off = f.len();
        for n in [0u64, 2u64] {
            f.extend_from_slice(&n.to_le_bytes());
        } // blob len 2 bytes
        let ididx_blob_off = f.len();
        let mut tmp = Vec::new();
        push_uvarint(1, &mut tmp);
        push_uvarint(0, &mut tmp);
        f.extend_from_slice(&tmp);
        // header
        f[ididx_hdr_off..ididx_hdr_off + 8].copy_from_slice(&1u64.to_le_bytes());
        f[ididx_hdr_off + 8..ididx_hdr_off + 16]
            .copy_from_slice(&(ididx_offs_off as u64).to_le_bytes());
        f[ididx_hdr_off + 16..ididx_hdr_off + 24]
            .copy_from_slice(&(ididx_blob_off as u64).to_le_bytes());
        add_sec(
            SectionKind::IdxId2Gid,
            ididx_sec_off,
            f.len() - ididx_sec_off,
        );

        // IDX_GNAME2GID same
        let gnidx_sec_off = f.len();
        let gnidx_hdr_off = gnidx_sec_off;
        f.resize(f.len() + 24, 0);
        let gnidx_offs_off = f.len();
        for n in [0u64, 2u64] {
            f.extend_from_slice(&n.to_le_bytes());
        }
        let gnidx_blob_off = f.len();
        let mut tmp2 = Vec::new();
        push_uvarint(1, &mut tmp2);
        push_uvarint(0, &mut tmp2);
        f.extend_from_slice(&tmp2);
        f[gnidx_hdr_off..gnidx_hdr_off + 8].copy_from_slice(&1u64.to_le_bytes());
        f[gnidx_hdr_off + 8..gnidx_hdr_off + 16]
            .copy_from_slice(&(gnidx_offs_off as u64).to_le_bytes());
        f[gnidx_hdr_off + 16..gnidx_hdr_off + 24]
            .copy_from_slice(&(gnidx_blob_off as u64).to_le_bytes());
        add_sec(
            SectionKind::IdxGName2Gid,
            gnidx_sec_off,
            f.len() - gnidx_sec_off,
        );

        // IDX_PAIR2GID with one pair (0,0)->0
        let pairidx_sec_off = f.len();
        let pairs_off = f.len() + 16; // header is 16 bytes
        // header
        f.extend_from_slice(&1u64.to_le_bytes());
        f.extend_from_slice(&(pairs_off as u64).to_le_bytes());
        // entry
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(&0u64.to_le_bytes());
        add_sec(
            SectionKind::IdxPair2Gid,
            pairidx_sec_off,
            f.len() - pairidx_sec_off,
        );

        // TOC
        let toc_off = f.len();
        for (kind, off, len) in &toc_entries {
            let mut ent = [0u8; 32];
            let kind_u16 = *kind as u16;
            ent[0..2].copy_from_slice(&kind_u16.to_le_bytes());
            ent[4..12].copy_from_slice(&off.to_le_bytes());
            ent[12..20].copy_from_slice(&len.to_le_bytes());
            // crc and reserved left zero
            f.extend_from_slice(&ent);
        }

        // Header
        f[0..4].copy_from_slice(b"R5TU");
        f[4..6].copy_from_slice(&1u16.to_le_bytes());
        f[6..8].copy_from_slice(&0u16.to_le_bytes());
        f[8..16].copy_from_slice(&0u64.to_le_bytes()); // created
        f[16..24].copy_from_slice(&(toc_off as u64).to_le_bytes());
        f[24..28].copy_from_slice(&(toc_entries.len() as u32).to_le_bytes());
        f[28..32].copy_from_slice(&0u32.to_le_bytes());

        // Write and open
        let mut path = std::env::temp_dir();
        path.push("mini.r5tu");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(&f).unwrap();
        let reader = R5tuFile::open(&path).unwrap();
        // enumerate by id
        let v = reader.enumerate_by_id("A").unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "A");
        assert_eq!(v[0].graphname, "g");
        assert_eq!(v[0].n_triples, 2);
        // enumerate by graphname
        let w = reader.enumerate_by_graphname("g").unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].id, "A");
        // resolve pair
        let gr = reader.resolve_gid("A", "g").unwrap().unwrap();
        assert_eq!(gr.gid, 0);
        // triples
        let triples: Vec<_> = reader.triples_ids(gr.gid).unwrap().collect();
        assert_eq!(triples, vec![(1, 2, 3), (1, 4, 5)]);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rejects_overlapping_toc_sections() {
        // Build two fake sections that overlap in TOC
        let mut f = vec![0u8; 32];
        // add a blob region
        let s1_off = f.len();
        f.extend_from_slice(&[0u8; 100]);
        let s2_off = s1_off + 50; // intentional overlap
        f.extend_from_slice(&[0u8; 20]);
        // TOC
        let toc_off = f.len();
        let mut ent1 = [0u8; 32];
        ent1[0..2].copy_from_slice(&(SectionKind::IdDict as u16).to_le_bytes());
        ent1[4..12].copy_from_slice(&(s1_off as u64).to_le_bytes());
        ent1[12..20].copy_from_slice(&(100u64).to_le_bytes());
        let mut ent2 = [0u8; 32];
        ent2[0..2].copy_from_slice(&(SectionKind::GNameDict as u16).to_le_bytes());
        ent2[4..12].copy_from_slice(&(s2_off as u64).to_le_bytes());
        ent2[12..20].copy_from_slice(&(20u64).to_le_bytes());
        f.extend_from_slice(&ent1);
        f.extend_from_slice(&ent2);
        // Header
        f[0..4].copy_from_slice(b"R5TU");
        f[4..6].copy_from_slice(&1u16.to_le_bytes());
        f[6..8].copy_from_slice(&0u16.to_le_bytes());
        f[8..16].copy_from_slice(&0u64.to_le_bytes());
        f[16..24].copy_from_slice(&(toc_off as u64).to_le_bytes());
        f[24..28].copy_from_slice(&(2u32).to_le_bytes());
        f[28..32].copy_from_slice(&0u32.to_le_bytes());

        let mut path = std::env::temp_dir();
        path.push("overlap.r5tu");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(&f).unwrap();
        let err = R5tuFile::open(&path).unwrap_err();
        match err {
            R5Error::Corrupt(m) => assert!(m.contains("overlap")),
            _ => panic!("expected Corrupt overlap"),
        }
        let _ = fs::remove_file(&path);
    }
}
