use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::header::{Section, SectionKind, TocEntry, crc32_ieee};
use crate::reader::{R5Error, Result};

// Simple type aliases to reduce type complexity noise
type GroupKey = (u32, u32);
type TripleIds = (u64, u64, u64);
type GroupsMap = BTreeMap<GroupKey, Vec<TripleIds>>;
type GidRow = (u32, u32, Section, u64, u32, u32, u32);
type PairEntry = (u32, u32, u64);

/// RDF term used by the writer when constructing quads.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Term {
    /// IRI/URI node.
    Iri(String),
    /// Blank node label (with or without `_:` prefix).
    BNode(String),
    /// Literal with optional datatype or language tag.
    Literal {
        lex: String,
        dt: Option<String>,
        lang: Option<String>,
    },
}

/// 5‑tuple (id, s, p, o, gname) used to build an R5TU file.
#[derive(Debug, Clone)]
pub struct Quint {
    /// Dataset identifier for grouping.
    pub id: String,
    /// Subject term.
    pub s: Term,
    /// Predicate term.
    pub p: Term,
    /// Object term.
    pub o: Term,
    /// Graph name for grouping.
    pub gname: String,
}

fn push_uvarint(mut v: u64, out: &mut Vec<u8>) {
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

/// Options controlling file emission.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriterOptions {
    /// Compress triple blocks using zstd (requires `zstd` feature).
    pub zstd: bool,
    /// Compute and embed per‑section CRCs (TOC) and a global footer CRC.
    pub with_crc: bool,
}

/// Convenience helper to write a `.r5tu` file with defaults.
///
/// - `zstd = false`
/// - `with_crc = true`
///
/// ```no_run
/// use rdf5d::{write_file, Quint, Term};
/// let q = Quint {
///     id: "dataset:1".into(),
///     s: Term::Iri("http://example.org/Alice".into()),
///     p: Term::Iri("http://xmlns.com/foaf/0.1/name".into()),
///     o: Term::Literal { lex: "Alice".into(), dt: None, lang: None },
///     gname: "http://example.org/graph".into(),
/// };
/// write_file("example.r5tu", &[q]).unwrap();
/// ```
pub fn write_file<P: AsRef<Path>>(path: P, quads: &[Quint]) -> Result<()> {
    write_file_with_options(
        path,
        quads,
        WriterOptions {
            zstd: false,
            with_crc: true,
        },
    )
}

/// Write a `.r5tu` file with explicit [`WriterOptions`].
pub fn write_file_with_options<P: AsRef<Path>>(
    path: P,
    quads: &[Quint],
    opts: WriterOptions,
) -> Result<()> {
    // 1) Deduplicate ids, gnames, terms
    let mut id_map: BTreeMap<String, u32> = BTreeMap::new();
    let mut gn_map: BTreeMap<String, u32> = BTreeMap::new();
    let mut term_map: HashMap<Term, u64> = HashMap::new();
    let mut id_vec: Vec<String> = Vec::new();
    let mut gn_vec: Vec<String> = Vec::new();
    let mut term_vec: Vec<Term> = Vec::new();

    let mut triples: Vec<(u32, u32, u64, u64, u64)> = Vec::new();
    // (id_id, gn_id, s_id, p_id, o_id)

    let mut intern_id = |s: &str| -> u32 {
        if let Some(&v) = id_map.get(s) {
            return v;
        }
        let v = id_vec.len() as u32;
        id_vec.push(s.to_string());
        id_map.insert(s.to_string(), v);
        v
    };
    let mut intern_gn = |s: &str| -> u32 {
        if let Some(&v) = gn_map.get(s) {
            return v;
        }
        let v = gn_vec.len() as u32;
        gn_vec.push(s.to_string());
        gn_map.insert(s.to_string(), v);
        v
    };
    let mut intern_term = |t: &Term| -> u64 {
        if let Some(&v) = term_map.get(t) {
            return v;
        }
        let v = term_vec.len() as u64;
        term_vec.push(t.clone());
        term_map.insert(t.clone(), v);
        v
    };

    for q in quads {
        let id_id = intern_id(&q.id);
        let gn_id = intern_gn(&q.gname);
        let s_id = intern_term(&q.s);
        let p_id = intern_term(&q.p);
        let o_id = intern_term(&q.o);
        triples.push((id_id, gn_id, s_id, p_id, o_id));
    }

    // 2) Group by (id_id, gn_id) and sort SPO
    let mut groups: GroupsMap = BTreeMap::new();
    for (id_id, gn_id, s, p, o) in triples {
        groups.entry((id_id, gn_id)).or_default().push((s, p, o));
    }
    for v in groups.values_mut() {
        v.sort_unstable();
    }

    // Buffers for sections
    let mut file = vec![0u8; 32]; // header placeholder
    let mut toc: Vec<TocEntry> = Vec::new();

    // ID_DICT
    let id_sec = write_str_dict(&mut file, &id_vec)?;
    toc.push(TocEntry {
        kind: SectionKind::IdDict,
        section: id_sec,
        crc32_u32: 0,
    });
    // GNAME_DICT
    let gn_sec = write_str_dict(&mut file, &gn_vec)?;
    toc.push(TocEntry {
        kind: SectionKind::GNameDict,
        section: gn_sec,
        crc32_u32: 0,
    });
    // TERM_DICT
    let term_sec = write_term_dict(&mut file, &term_vec)?;
    toc.push(TocEntry {
        kind: SectionKind::TermDict,
        section: term_sec,
        crc32_u32: 0,
    });

    // TRIPLE_BLOCKS
    let tb_off = file.len();
    let mut gid_rows: Vec<GidRow> = Vec::new();
    // For stable GID ordering, iterate groups in key order (BTreeMap)
    for ((id_id, gn_id), spo) in &groups {
        let start = file.len();
        // build RAW payload for this group
        let raw = build_raw_spo(spo)?;
        if opts.zstd {
            #[cfg(feature = "zstd")]
            {
                file.push(1u8); // enc=ZSTD
                let compressed = zstd::encode_all(&raw[..], 0)
                    .map_err(|_| R5Error::Corrupt("zstd encode".into()))?;
                let clen = compressed.len() as u32;
                file.extend_from_slice(&clen.to_le_bytes());
                file.extend_from_slice(&compressed);
            }
            #[cfg(not(feature = "zstd"))]
            {
                return Err(R5Error::Invalid("zstd feature not enabled"));
            }
        } else {
            // RAW
            file.push(0u8);
            let raw_len = raw.len() as u32;
            file.extend_from_slice(&raw_len.to_le_bytes());
            file.extend_from_slice(&raw);
        }
        let sec = Section {
            off: start as u64,
            len: (file.len() - start) as u64,
        };
        // counts
        let (n_s, n_p, n_t) = raw_counts(&raw)?;
        gid_rows.push((
            *id_id, *gn_id, sec, n_t as u64, n_s as u32, n_p as u32, n_t as u32,
        ));
    }
    let tb_sec = Section {
        off: tb_off as u64,
        len: (file.len() - tb_off) as u64,
    };
    toc.push(TocEntry {
        kind: SectionKind::TripleBlocks,
        section: tb_sec,
        crc32_u32: 0,
    });

    // GDIR
    let gdir_off = file.len();
    let n_rows = gid_rows.len() as u64;
    file.extend_from_slice(&n_rows.to_le_bytes());
    file.extend_from_slice(&44u32.to_le_bytes()); // row_size actually written below
    file.extend_from_slice(&0u32.to_le_bytes()); // reserved
    for (id_id, gn_id, sec, n_triples, n_s, n_p, n_o) in &gid_rows {
        file.extend_from_slice(&id_id.to_le_bytes());
        file.extend_from_slice(&gn_id.to_le_bytes());
        file.extend_from_slice(&sec.off.to_le_bytes());
        file.extend_from_slice(&sec.len.to_le_bytes());
        file.extend_from_slice(&n_triples.to_le_bytes());
        file.extend_from_slice(&n_s.to_le_bytes());
        file.extend_from_slice(&n_p.to_le_bytes());
        file.extend_from_slice(&n_o.to_le_bytes());
    }
    let gdir_sec = Section {
        off: gdir_off as u64,
        len: (file.len() - gdir_off) as u64,
    };
    toc.push(TocEntry {
        kind: SectionKind::GDir,
        section: gdir_sec,
        crc32_u32: 0,
    });

    // Build GID mapping for postings & pair index
    let mut pair_entries: Vec<PairEntry> = Vec::new();
    let mut id2gids: Vec<Vec<u64>> = vec![Vec::new(); id_vec.len()];
    let mut gn2gids: Vec<Vec<u64>> = vec![Vec::new(); gn_vec.len()];
    for (gid, (id_id, gn_id, _, _, _, _, _)) in gid_rows.iter().enumerate() {
        let gid_u = gid as u64;
        id2gids[*id_id as usize].push(gid_u);
        gn2gids[*gn_id as usize].push(gid_u);
        pair_entries.push((*id_id, *gn_id, gid_u));
    }
    pair_entries.sort_unstable();

    // IDX_ID2GID
    let ididx_sec = write_postings_index(&mut file, &id2gids)?;
    toc.push(TocEntry {
        kind: SectionKind::IdxId2Gid,
        section: ididx_sec,
        crc32_u32: 0,
    });
    // IDX_GNAME2GID
    let gnidx_sec = write_postings_index(&mut file, &gn2gids)?;
    toc.push(TocEntry {
        kind: SectionKind::IdxGName2Gid,
        section: gnidx_sec,
        crc32_u32: 0,
    });
    // IDX_PAIR2GID
    let pairidx_sec = write_pair_index(&mut file, &pair_entries)?;
    toc.push(TocEntry {
        kind: SectionKind::IdxPair2Gid,
        section: pairidx_sec,
        crc32_u32: 0,
    });

    // TOC
    let toc_off = file.len();
    for e in &toc {
        let mut ent = [0u8; 32];
        let kind = e.kind as u16;
        ent[0..2].copy_from_slice(&kind.to_le_bytes());
        // reserved_u16 zero
        ent[4..12].copy_from_slice(&e.section.off.to_le_bytes());
        ent[12..20].copy_from_slice(&e.section.len.to_le_bytes());
        if opts.with_crc {
            let start = e.section.off as usize;
            let end = start + e.section.len as usize;
            let crc = crc32_ieee(&file[start..end]);
            ent[20..24].copy_from_slice(&crc.to_le_bytes());
        }
        file.extend_from_slice(&ent);
    }

    // Header
    file[0..4].copy_from_slice(b"R5TU");
    file[4..6].copy_from_slice(&1u16.to_le_bytes()); // version
    let mut flags: u16 = 0;
    if opts.zstd {
        flags |= 1 << 1;
    }
    file[6..8].copy_from_slice(&flags.to_le_bytes());
    file[8..16].copy_from_slice(&0u64.to_le_bytes()); // created
    file[16..24].copy_from_slice(&(toc_off as u64).to_le_bytes());
    file[24..28].copy_from_slice(&(toc.len() as u32).to_le_bytes());
    file[28..32].copy_from_slice(&0u32.to_le_bytes());

    // Footer with global CRC
    let crc = crc32_ieee(&file[..]);
    file.extend_from_slice(&crc.to_le_bytes());
    file.extend_from_slice(b"R5TU_ENDMARK");

    // Atomic write (best-effort)
    let tmp_path = path.as_ref().with_extension(".tmp.r5tu");
    fs::write(&tmp_path, &file).map_err(R5Error::Io)?;
    fs::rename(&tmp_path, path).map_err(R5Error::Io)?;
    Ok(())
}

// ---------------- Streaming writer ----------------
/// Incremental builder for large datasets.
///
/// Use [`StreamingWriter::add`] to append quads, then [`StreamingWriter::finalize`]
/// to write the file atomically.
#[derive(Debug)]
pub struct StreamingWriter {
    opts: WriterOptions,
    path: PathBuf,
    id_map: BTreeMap<String, u32>,
    gn_map: BTreeMap<String, u32>,
    term_map: HashMap<Term, u64>,
    id_vec: Vec<String>,
    gn_vec: Vec<String>,
    term_vec: Vec<Term>,
    groups: GroupsMap,
}

impl StreamingWriter {
    /// Create a streaming writer targeting `path` with `opts`.
    pub fn new<P: Into<PathBuf>>(path: P, opts: WriterOptions) -> Self {
        Self {
            opts,
            path: path.into(),
            id_map: BTreeMap::new(),
            gn_map: BTreeMap::new(),
            term_map: HashMap::new(),
            id_vec: Vec::new(),
            gn_vec: Vec::new(),
            term_vec: Vec::new(),
            groups: BTreeMap::new(),
        }
    }

    fn intern_id(&mut self, s: &str) -> u32 {
        if let Some(&v) = self.id_map.get(s) {
            return v;
        }
        let v = self.id_vec.len() as u32;
        self.id_vec.push(s.to_string());
        self.id_map.insert(s.to_string(), v);
        v
    }
    fn intern_gn(&mut self, s: &str) -> u32 {
        if let Some(&v) = self.gn_map.get(s) {
            return v;
        }
        let v = self.gn_vec.len() as u32;
        self.gn_vec.push(s.to_string());
        self.gn_map.insert(s.to_string(), v);
        v
    }
    fn intern_term(&mut self, t: &Term) -> u64 {
        if let Some(&v) = self.term_map.get(t) {
            return v;
        }
        let v = self.term_vec.len() as u64;
        self.term_vec.push(t.clone());
        self.term_map.insert(t.clone(), v);
        v
    }

    /// Add one 5‑tuple to the in‑memory builder.
    pub fn add(&mut self, q: Quint) -> Result<()> {
        let id_id = self.intern_id(&q.id);
        let gn_id = self.intern_gn(&q.gname);
        let s = self.intern_term(&q.s);
        let p = self.intern_term(&q.p);
        let o = self.intern_term(&q.o);
        self.groups
            .entry((id_id, gn_id))
            .or_default()
            .push((s, p, o));
        Ok(())
    }

    /// Finish building and write the file to disk.
    pub fn finalize(mut self) -> Result<()> {
        // Ensure per-group SPO sort
        for v in self.groups.values_mut() {
            v.sort_unstable();
        }

        // Build buffers using the same logic as write_file_with_options
        let mut file = vec![0u8; 32];
        let mut toc: Vec<TocEntry> = Vec::new();

        let id_sec = write_str_dict(&mut file, &self.id_vec)?;
        toc.push(TocEntry {
            kind: SectionKind::IdDict,
            section: id_sec,
            crc32_u32: 0,
        });
        let gn_sec = write_str_dict(&mut file, &self.gn_vec)?;
        toc.push(TocEntry {
            kind: SectionKind::GNameDict,
            section: gn_sec,
            crc32_u32: 0,
        });
        let term_sec = write_term_dict(&mut file, &self.term_vec)?;
        toc.push(TocEntry {
            kind: SectionKind::TermDict,
            section: term_sec,
            crc32_u32: 0,
        });

        let tb_off = file.len();
        let mut gid_rows: Vec<GidRow> = Vec::new();
        for ((id_id, gn_id), spo) in &self.groups {
            let start = file.len();
            let raw = build_raw_spo(spo)?;
            if self.opts.zstd {
                #[cfg(feature = "zstd")]
                {
                    file.push(1u8);
                    let compressed = zstd::encode_all(&raw[..], 0)
                        .map_err(|_| R5Error::Corrupt("zstd encode".into()))?;
                    file.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
                    file.extend_from_slice(&compressed);
                }
                #[cfg(not(feature = "zstd"))]
                {
                    return Err(R5Error::Invalid("zstd feature not enabled"));
                }
            } else {
                file.push(0u8);
                file.extend_from_slice(&(raw.len() as u32).to_le_bytes());
                file.extend_from_slice(&raw);
            }
            let sec = Section {
                off: start as u64,
                len: (file.len() - start) as u64,
            };
            let (n_s, n_p, n_t) = raw_counts(&raw)?;
            gid_rows.push((
                *id_id, *gn_id, sec, n_t as u64, n_s as u32, n_p as u32, n_t as u32,
            ));
        }
        let tb_sec = Section {
            off: tb_off as u64,
            len: (file.len() - tb_off) as u64,
        };
        toc.push(TocEntry {
            kind: SectionKind::TripleBlocks,
            section: tb_sec,
            crc32_u32: 0,
        });

        // GDIR
        let gdir_off = file.len();
        let n_rows = gid_rows.len() as u64;
        file.extend_from_slice(&n_rows.to_le_bytes());
        file.extend_from_slice(&44u32.to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        for (id_id, gn_id, sec, n_triples, n_s, n_p, n_o) in &gid_rows {
            file.extend_from_slice(&id_id.to_le_bytes());
            file.extend_from_slice(&gn_id.to_le_bytes());
            file.extend_from_slice(&sec.off.to_le_bytes());
            file.extend_from_slice(&sec.len.to_le_bytes());
            file.extend_from_slice(&n_triples.to_le_bytes());
            file.extend_from_slice(&n_s.to_le_bytes());
            file.extend_from_slice(&n_p.to_le_bytes());
            file.extend_from_slice(&n_o.to_le_bytes());
        }
        let gdir_sec = Section {
            off: gdir_off as u64,
            len: (file.len() - gdir_off) as u64,
        };
        toc.push(TocEntry {
            kind: SectionKind::GDir,
            section: gdir_sec,
            crc32_u32: 0,
        });

        // Postings and pair index
        let mut pair_entries: Vec<PairEntry> = Vec::new();
        let mut id2gids: Vec<Vec<u64>> = vec![Vec::new(); self.id_vec.len()];
        let mut gn2gids: Vec<Vec<u64>> = vec![Vec::new(); self.gn_vec.len()];
        for (gid, (id_id, gn_id, _, _, _, _, _)) in gid_rows.iter().enumerate() {
            let gid_u = gid as u64;
            id2gids[*id_id as usize].push(gid_u);
            gn2gids[*gn_id as usize].push(gid_u);
            pair_entries.push((*id_id, *gn_id, gid_u));
        }
        pair_entries.sort_unstable();

        let ididx_sec = write_postings_index(&mut file, &id2gids)?;
        toc.push(TocEntry {
            kind: SectionKind::IdxId2Gid,
            section: ididx_sec,
            crc32_u32: 0,
        });
        let gnidx_sec = write_postings_index(&mut file, &gn2gids)?;
        toc.push(TocEntry {
            kind: SectionKind::IdxGName2Gid,
            section: gnidx_sec,
            crc32_u32: 0,
        });
        let pairidx_sec = write_pair_index(&mut file, &pair_entries)?;
        toc.push(TocEntry {
            kind: SectionKind::IdxPair2Gid,
            section: pairidx_sec,
            crc32_u32: 0,
        });

        // TOC
        let toc_off = file.len();
        for e in &toc {
            let mut ent = [0u8; 32];
            let kind = e.kind as u16;
            ent[0..2].copy_from_slice(&kind.to_le_bytes());
            ent[4..12].copy_from_slice(&e.section.off.to_le_bytes());
            ent[12..20].copy_from_slice(&e.section.len.to_le_bytes());
            if self.opts.with_crc {
                let start = e.section.off as usize;
                let end = start + e.section.len as usize;
                let crc = crc32_ieee(&file[start..end]);
                ent[20..24].copy_from_slice(&crc.to_le_bytes());
            }
            file.extend_from_slice(&ent);
        }

        // Header
        file[0..4].copy_from_slice(b"R5TU");
        file[4..6].copy_from_slice(&1u16.to_le_bytes());
        let mut flags: u16 = 0;
        if self.opts.zstd {
            flags |= 1 << 1;
        }
        file[6..8].copy_from_slice(&flags.to_le_bytes());
        file[8..16].copy_from_slice(&0u64.to_le_bytes());
        file[16..24].copy_from_slice(&(toc_off as u64).to_le_bytes());
        file[24..28].copy_from_slice(&(toc.len() as u32).to_le_bytes());
        file[28..32].copy_from_slice(&0u32.to_le_bytes());

        // Footer
        let crc = crc32_ieee(&file[..]);
        file.extend_from_slice(&crc.to_le_bytes());
        file.extend_from_slice(b"R5TU_ENDMARK");

        // Write
        let tmp = self.path.with_extension(".tmp.r5tu");
        fs::write(&tmp, &file).map_err(R5Error::Io)?;
        fs::rename(&tmp, &self.path).map_err(R5Error::Io)?;
        Ok(())
    }
}

// ---------------- Oxigraph helpers ----------------

#[cfg(feature = "oxigraph")]
fn term_from_ox_term_ref(t: &oxigraph::model::TermRef<'_>) -> Term {
    use oxigraph::model::TermRef as TR;
    match t {
        TR::NamedNode(n) => Term::Iri(n.as_str().to_string()),
        TR::BlankNode(b) => Term::BNode(format!("_:{}", b.as_str())),
        TR::Literal(l) => {
            let lex = l.value().to_string();
            if let Some(lang) = l.language() {
                Term::Literal {
                    lex,
                    dt: None,
                    lang: Some(lang.to_string()),
                }
            } else {
                Term::Literal {
                    lex,
                    dt: Some(l.datatype().as_str().to_string()),
                    lang: None,
                }
            }
        }
        _ => Term::Iri(t.to_string()),
    }
}

#[cfg(feature = "oxigraph")]
impl StreamingWriter {
    pub fn add_oxigraph_graph(
        &mut self,
        graph: &oxigraph::model::Graph,
        id: &str,
        gname: &str,
    ) -> Result<()> {
        use oxigraph::model::SubjectRef;
        for t in graph.iter() {
            let s = match &t.subject {
                SubjectRef::NamedNode(n) => Term::Iri(n.as_str().to_string()),
                SubjectRef::BlankNode(b) => Term::BNode(format!("_:{}", b.as_str())),
                _ => return Err(R5Error::Invalid("unsupported subject kind")),
            };
            let p = Term::Iri(t.predicate.as_str().to_string());
            let o = term_from_ox_term_ref(&t.object);
            self.add(Quint {
                id: id.to_string(),
                s,
                p,
                o,
                gname: gname.to_string(),
            })?;
        }
        Ok(())
    }
}

#[cfg(feature = "oxigraph")]
pub fn write_graph_from_oxigraph<P: AsRef<Path>>(
    path: P,
    graph: &oxigraph::model::Graph,
    id: &str,
    gname: &str,
    opts: WriterOptions,
) -> Result<()> {
    let mut w = StreamingWriter::new(path.as_ref(), opts);
    w.add_oxigraph_graph(graph, id, gname)?;
    w.finalize()
}

#[cfg(feature = "oxigraph")]
pub fn detect_graphname_from_oxigraph(graph: &oxigraph::model::Graph) -> Option<String> {
    use oxigraph::model::{NamedNode, SubjectRef, TermRef};
    let rdf_type = NamedNode::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type").ok()?;
    let owl_ontology = NamedNode::new("http://www.w3.org/2002/07/owl#Ontology").ok()?;
    for t in graph.iter() {
        if t.predicate == rdf_type.as_ref() && t.object == TermRef::NamedNode(owl_ontology.as_ref())
        {
            return Some(match t.subject {
                SubjectRef::NamedNode(n) => n.as_str().to_string(),
                SubjectRef::BlankNode(b) => format!("_:{}", b.as_str()),
                _ => return None,
            });
        }
    }
    None
}

#[cfg(feature = "oxigraph")]
pub fn write_graph_from_oxigraph_auto<P: AsRef<Path>>(
    path: P,
    graph: &oxigraph::model::Graph,
    opts: WriterOptions,
) -> Result<()> {
    let gname = detect_graphname_from_oxigraph(graph).unwrap_or_else(|| "default".to_string());
    write_graph_from_oxigraph(path, graph, "0", &gname, opts)
}

#[cfg(feature = "oxigraph")]
pub fn detect_graphname_from_store(store: &oxigraph::store::Store) -> Option<String> {
    use oxigraph::model::{GraphNameRef, NamedNode, TermRef};
    let rdf_type = NamedNode::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type").ok()?;
    let owl_ontology = NamedNode::new("http://www.w3.org/2002/07/owl#Ontology").ok()?;
    let mut it = store.quads_for_pattern(
        None,
        Some(rdf_type.as_ref()),
        Some(TermRef::NamedNode(owl_ontology.as_ref())),
        Some(GraphNameRef::DefaultGraph),
    );
    if let Some(Ok(q)) = it.next() {
        return Some(match &q.subject {
            oxigraph::model::Subject::NamedNode(n) => n.as_str().to_string(),
            oxigraph::model::Subject::BlankNode(b) => format!("_:{}", b.as_str()),
            _ => return None,
        });
    }
    None
}

fn write_str_dict(buf: &mut Vec<u8>, strings: &[String]) -> Result<Section> {
    let off = buf.len();
    // header 52 bytes
    buf.resize(buf.len() + 52, 0);
    let blob_off = buf.len();
    for s in strings {
        buf.extend_from_slice(s.as_bytes());
    }
    let blob_len = buf.len() - blob_off;
    let offs_off = buf.len();
    // offs len = (n+1) * 4
    let mut cur = 0u32;
    for s in strings {
        buf.extend_from_slice(&cur.to_le_bytes());
        cur = cur
            .checked_add(s.len() as u32)
            .ok_or_else(|| R5Error::Corrupt("blob size".into()))?;
    }
    buf.extend_from_slice(&cur.to_le_bytes());
    let offs_len = buf.len() - offs_off;
    // build coarse index (key16 + id + padding) entries sorted by key16 then id
    let mut idx_entries: Vec<([u8; 16], u32)> = Vec::with_capacity(strings.len());
    for (i, s) in strings.iter().enumerate() {
        let mut key = [0u8; 16];
        for (j, b) in s
            .to_ascii_lowercase()
            .as_bytes()
            .iter()
            .take(16)
            .enumerate()
        {
            key[j] = *b;
        }
        idx_entries.push((key, i as u32));
    }
    idx_entries.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let idx_off;
    let idx_len;
    if !idx_entries.is_empty() {
        idx_off = buf.len();
        for (key, id) in idx_entries {
            buf.extend_from_slice(&key);
            buf.extend_from_slice(&id.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes()); // padding to 24 bytes
        }
        idx_len = buf.len() - idx_off;
    } else {
        idx_off = 0;
        idx_len = 0;
    }
    // fill header
    let n = strings.len() as u32;
    buf[off..off + 4].copy_from_slice(&n.to_le_bytes());
    buf[off + 4..off + 12].copy_from_slice(&(blob_off as u64).to_le_bytes());
    buf[off + 12..off + 20].copy_from_slice(&(blob_len as u64).to_le_bytes());
    buf[off + 20..off + 28].copy_from_slice(&(offs_off as u64).to_le_bytes());
    buf[off + 28..off + 36].copy_from_slice(&(offs_len as u64).to_le_bytes());
    buf[off + 36..off + 44].copy_from_slice(&(idx_off as u64).to_le_bytes());
    buf[off + 44..off + 52].copy_from_slice(&(idx_len as u64).to_le_bytes());
    Ok(Section {
        off: off as u64,
        len: (buf.len() - off) as u64,
    })
}

fn write_term_dict(buf: &mut Vec<u8>, terms: &[Term]) -> Result<Section> {
    let off = buf.len();
    // header 33 bytes
    buf.resize(buf.len() + 33, 0);
    // kinds
    let kinds_off = buf.len();
    for t in terms {
        buf.push(match t {
            Term::Iri(_) => 0,
            Term::BNode(_) => 1,
            Term::Literal { .. } => 2,
        });
    }
    // data blob
    let data_off = buf.len();
    let mut offs: Vec<u64> = Vec::with_capacity(terms.len() + 1);
    let mut cur: u64 = 0;
    offs.push(cur);
    for t in terms {
        match t {
            Term::Iri(s) | Term::BNode(s) => {
                buf.extend_from_slice(s.as_bytes());
                cur += s.len() as u64;
            }
            Term::Literal { lex, dt, lang } => {
                push_uvarint(lex.len() as u64, buf);
                buf.extend_from_slice(lex.as_bytes());
                match dt {
                    Some(d) => {
                        buf.push(1);
                        push_uvarint(d.len() as u64, buf);
                        buf.extend_from_slice(d.as_bytes());
                    }
                    None => buf.push(0),
                }
                match lang {
                    Some(l) => {
                        buf.push(1);
                        push_uvarint(l.len() as u64, buf);
                        buf.extend_from_slice(l.as_bytes());
                    }
                    None => buf.push(0),
                }
                cur = (buf.len() - data_off) as u64;
            }
        }
        offs.push(cur);
    }
    // offs u64*(n+1)
    let offs_off = buf.len();
    for o in offs {
        buf.extend_from_slice(&o.to_le_bytes());
    }

    // fill header
    buf[off] = 0; // width
    buf[off + 1..off + 9].copy_from_slice(&(terms.len() as u64).to_le_bytes());
    buf[off + 9..off + 17].copy_from_slice(&(kinds_off as u64).to_le_bytes());
    buf[off + 17..off + 25].copy_from_slice(&(data_off as u64).to_le_bytes());
    buf[off + 25..off + 33].copy_from_slice(&(offs_off as u64).to_le_bytes());
    Ok(Section {
        off: off as u64,
        len: (buf.len() - off) as u64,
    })
}

fn build_raw_spo(spo: &[(u64, u64, u64)]) -> Result<Vec<u8>> {
    // Precondition: spo sorted by (s,p,o)
    let n_t = spo.len();
    let mut out = Vec::with_capacity(n_t * 2);
    // collect unique S and P structure
    let mut s_vals: Vec<u64> = Vec::new();
    let mut s_heads: Vec<u64> = Vec::new();
    let mut p_vals: Vec<u64> = Vec::new();
    let mut p_heads: Vec<u64> = Vec::new();
    let mut o_vals: Vec<u64> = Vec::new();

    let mut i = 0usize;
    while i < spo.len() {
        let s = spo[i].0;
        s_vals.push(s);
        s_heads.push(p_vals.len() as u64);
        // group by s
        let mut j = i;
        while j < spo.len() {
            if spo[j].0 != s {
                break;
            }
            // new p run
            let p = spo[j].1;
            p_vals.push(p);
            p_heads.push(o_vals.len() as u64);
            // group by (s,p)
            let mut k = j;
            while k < spo.len() && spo[k].0 == s && spo[k].1 == p {
                o_vals.push(spo[k].2);
                k += 1;
            }
            j = k;
        }
        i = j;
    }
    s_heads.push(p_vals.len() as u64);
    p_heads.push(o_vals.len() as u64);

    // nS, nP, nT
    push_uvarint(s_vals.len() as u64, &mut out);
    push_uvarint(p_vals.len() as u64, &mut out);
    push_uvarint(o_vals.len() as u64, &mut out);
    // S_vals delta-coded
    if !s_vals.is_empty() {
        let mut prev = 0u64;
        for (idx, v) in s_vals.iter().enumerate() {
            if idx == 0 {
                push_uvarint(*v, &mut out);
                prev = *v;
            } else {
                push_uvarint(
                    v.checked_sub(prev)
                        .ok_or_else(|| R5Error::Corrupt("s delta underflow".into()))?,
                    &mut out,
                );
                prev = *v;
            }
        }
    }
    // S_heads
    for v in &s_heads {
        push_uvarint(*v, &mut out);
    }
    // P_vals delta-coded per S-run
    for s_idx in 0..s_vals.len() {
        let start = s_heads[s_idx] as usize;
        let end = s_heads[s_idx + 1] as usize;
        if start == end {
            continue;
        }
        let mut prev = 0u64;
        for (i, idx) in (start..end).enumerate() {
            let v = p_vals[idx];
            if i == 0 {
                push_uvarint(v, &mut out);
                prev = v;
            } else {
                push_uvarint(
                    v.checked_sub(prev)
                        .ok_or_else(|| R5Error::Corrupt("p delta underflow".into()))?,
                    &mut out,
                );
                prev = v;
            }
        }
    }
    // P_heads
    for v in &p_heads {
        push_uvarint(*v, &mut out);
    }
    // O_vals delta-coded per (S,P)-run
    for p_idx in 0..p_vals.len() {
        let start = p_heads[p_idx] as usize;
        let end = p_heads[p_idx + 1] as usize;
        if start == end {
            continue;
        }
        let mut prev = 0u64;
        for (i, idx) in (start..end).enumerate() {
            let v = o_vals[idx];
            if i == 0 {
                push_uvarint(v, &mut out);
                prev = v;
            } else {
                push_uvarint(
                    v.checked_sub(prev)
                        .ok_or_else(|| R5Error::Corrupt("o delta underflow".into()))?,
                    &mut out,
                );
                prev = v;
            }
        }
    }
    Ok(out)
}

fn raw_counts(raw: &[u8]) -> Result<(usize, usize, usize)> {
    let (n_s, o1) = read_uvarint(raw, 0).ok_or_else(|| R5Error::Corrupt("nS".into()))?;
    let (n_p, o2) = read_uvarint(raw, o1).ok_or_else(|| R5Error::Corrupt("nP".into()))?;
    let (n_t, _) = read_uvarint(raw, o2).ok_or_else(|| R5Error::Corrupt("nT".into()))?;
    Ok((n_s as usize, n_p as usize, n_t as usize))
}

fn write_postings_index(buf: &mut Vec<u8>, lists: &[Vec<u64>]) -> Result<Section> {
    let off = buf.len();
    buf.resize(buf.len() + 24, 0); // header
    let offs_off = buf.len();
    let mut cur = 0u64;
    buf.extend_from_slice(&cur.to_le_bytes());
    let mut blob = Vec::new();
    for list in lists {
        // encode list
        if list.is_empty() {
            push_uvarint(0, &mut blob);
        } else {
            push_uvarint(list.len() as u64, &mut blob);
            push_uvarint(list[0], &mut blob);
            for w in list.windows(2) {
                push_uvarint(w[1] - w[0], &mut blob);
            }
        }
        cur += blob.len() as u64 - cur;
        buf.extend_from_slice(&(blob.len() as u64).to_le_bytes());
    }
    let blob_off = buf.len();
    buf.extend_from_slice(&blob);
    // fill header
    buf[off..off + 8].copy_from_slice(&(lists.len() as u64).to_le_bytes());
    buf[off + 8..off + 16].copy_from_slice(&(offs_off as u64).to_le_bytes());
    buf[off + 16..off + 24].copy_from_slice(&(blob_off as u64).to_le_bytes());
    Ok(Section {
        off: off as u64,
        len: (buf.len() - off) as u64,
    })
}

fn write_pair_index(buf: &mut Vec<u8>, pairs: &[(u32, u32, u64)]) -> Result<Section> {
    let off = buf.len();
    buf.extend_from_slice(&(pairs.len() as u64).to_le_bytes());
    let pairs_off = buf.len() + 8; // we will place entries after writing pairs_off
    buf.extend_from_slice(&(pairs_off as u64).to_le_bytes());
    for (id_id, gn_id, gid) in pairs {
        buf.extend_from_slice(&id_id.to_le_bytes());
        buf.extend_from_slice(&gn_id.to_le_bytes());
        buf.extend_from_slice(&gid.to_le_bytes());
    }
    Ok(Section {
        off: off as u64,
        len: (buf.len() - off) as u64,
    })
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
