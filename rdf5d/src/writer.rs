use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::header::{Section, SectionKind, TocEntry, crc32_ieee};
use crate::reader::{R5Error, Result};

// Simple type aliases to reduce type complexity noise
type GroupKey = (u32, u32);
type TripleIds = (u64, u64, u64);
type GroupsMap = BTreeMap<GroupKey, Vec<TripleIds>>;
type GidRow = (u32, u32, Section, u64, u32, u32, u32);
type PairEntry = (u32, u32, u64);

const TERM_DICT_FLAG_LITERAL_COMPONENTS: u8 = 0x80;
const TERM_DICT_WIDTH_MASK: u8 = 0x0f;
const STR_DICT_OFFS_FLAG_FRONT_CODED: u64 = 1 << 63;
const STR_DICT_IDX_FLAG_GROUPED: u64 = 1 << 63;
const STR_DICT_FRONT_BLOCK_SIZE: usize = 16;
const TERM_KIND_IRI: u8 = 0;
const TERM_KIND_BNODE: u8 = 1;
const TERM_KIND_LITERAL_INLINE: u8 = 2;
const TERM_KIND_LITERAL_COMPONENTS: u8 = 3;

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: both paths are valid, NUL-terminated UTF-16 buffers which live
    // for the duration of the call.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Durably replace `path` with `bytes` while preserving the previous valid
/// destination on every failure before the atomic rename.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_before_replace(path, bytes, || Ok(()))
}

fn atomic_write_before_replace(
    path: &Path,
    bytes: &[u8],
    before_replace: impl FnOnce() -> std::io::Result<()>,
) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(R5Error::Io)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("rdf5d");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let mut temporary = None;
    for attempt in 0..100u32 {
        let candidate = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            nonce + u128::from(attempt)
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(R5Error::Io(error)),
        }
    }
    let (temporary_path, mut temporary_file) = temporary.ok_or_else(|| {
        R5Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique RDF5D temporary file",
        ))
    })?;

    let result = (|| {
        temporary_file.write_all(bytes).map_err(R5Error::Io)?;
        temporary_file.flush().map_err(R5Error::Io)?;
        temporary_file.sync_all().map_err(R5Error::Io)?;
        drop(temporary_file);
        before_replace().map_err(R5Error::Io)?;
        atomic_replace(&temporary_path, path).map_err(R5Error::Io)?;
        #[cfg(unix)]
        {
            fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(R5Error::Io)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[derive(Debug)]
struct RawSpoBuild {
    raw: Vec<u8>,
    n_s: u32,
    n_p: u32,
    n_t: u32,
}

#[derive(Debug, Default)]
struct LiteralComponentPool {
    lex: Vec<String>,
    dt: Vec<String>,
    lang: Vec<String>,
    lex_map: HashMap<String, u32>,
    dt_map: HashMap<String, u32>,
    lang_map: HashMap<String, u32>,
}

#[derive(Debug)]
struct TermDataBuild {
    kinds: Vec<u8>,
    data: Vec<u8>,
    offs: Vec<u64>,
    literal_components: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EncodedQuint {
    id_id: u32,
    gn_id: u32,
    s: u64,
    p: u64,
    o: u64,
}

fn compact_gdir_row_size(rows: &[GidRow]) -> u32 {
    if rows.iter().all(|(_, _, sec, n_triples, _, _, _)| {
        u32::try_from(sec.off).is_ok()
            && u32::try_from(sec.len).is_ok()
            && u32::try_from(*n_triples).is_ok()
    }) {
        32
    } else {
        44
    }
}

fn write_gdir_rows(buf: &mut Vec<u8>, rows: &[GidRow]) {
    let row_size = compact_gdir_row_size(rows);
    buf.extend_from_slice(&(rows.len() as u64).to_le_bytes());
    buf.extend_from_slice(&row_size.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    for (id_id, gn_id, sec, n_triples, n_s, n_p, n_o) in rows {
        buf.extend_from_slice(&id_id.to_le_bytes());
        buf.extend_from_slice(&gn_id.to_le_bytes());
        if row_size == 32 {
            buf.extend_from_slice(&(sec.off as u32).to_le_bytes());
            buf.extend_from_slice(&(sec.len as u32).to_le_bytes());
            buf.extend_from_slice(&(*n_triples as u32).to_le_bytes());
            buf.extend_from_slice(&n_s.to_le_bytes());
            buf.extend_from_slice(&n_p.to_le_bytes());
            buf.extend_from_slice(&n_o.to_le_bytes());
        } else {
            buf.extend_from_slice(&sec.off.to_le_bytes());
            buf.extend_from_slice(&sec.len.to_le_bytes());
            buf.extend_from_slice(&n_triples.to_le_bytes());
            buf.extend_from_slice(&n_s.to_le_bytes());
            buf.extend_from_slice(&n_p.to_le_bytes());
            buf.extend_from_slice(&n_o.to_le_bytes());
        }
    }
}

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

fn uvarint_len(mut v: u64) -> usize {
    let mut len = 1usize;
    while v >= 0x80 {
        v >>= 7;
        len += 1;
    }
    len
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

/// Options controlling file emission.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriterOptions {
    /// Compress triple blocks using zstd (requires `zstd` feature).
    pub zstd: bool,
    /// Compute and embed per‑section CRCs (TOC) and a global footer CRC.
    pub with_crc: bool,
}

/// Summary statistics from a [`StreamingWriter`] build.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamingWriteStats {
    /// Total number of quads accepted by the writer.
    pub total_quads: usize,
    /// Target chunk size used by the writer.
    pub chunk_quads: usize,
    /// Maximum number of buffered quads held before a spill/finalize flush.
    pub max_pending_quads: usize,
    /// Number of sorted temporary runs written to disk.
    pub run_count: usize,
    /// Total bytes written to temporary run files.
    pub temp_bytes_written: u64,
}

/// Spill policy controlling when [`StreamingWriter`] flushes pending quads to a sorted run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpillPolicy {
    /// Choose a chunk size from the workload hint, bounded to a conservative default range.
    #[default]
    Auto,
    /// Spill once the pending in-memory encoded quads reach this count.
    MaxPendingQuads(usize),
    /// Spill once the pending in-memory encoded quads reach roughly this many bytes.
    TargetPendingBytes(usize),
}

/// Public configuration for [`StreamingWriter`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamingWriterOptions {
    /// File emission options shared with the batch writer.
    pub writer: WriterOptions,
    /// Spill policy for the external-sort pipeline.
    pub spill_policy: SpillPolicy,
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
    let estimated_unique_strings = quads.len().min(256);
    let mut id_map: HashMap<String, u32> = HashMap::with_capacity(estimated_unique_strings);
    let mut gn_map: HashMap<String, u32> = HashMap::with_capacity(estimated_unique_strings);
    let mut term_map: HashMap<Term, u64> = HashMap::with_capacity(quads.len().saturating_mul(3));
    let mut id_vec: Vec<String> = Vec::with_capacity(estimated_unique_strings);
    let mut gn_vec: Vec<String> = Vec::with_capacity(estimated_unique_strings);
    let mut term_vec: Vec<Term> = Vec::with_capacity(quads.len().saturating_mul(3));

    let mut triples: Vec<(u32, u32, u64, u64, u64)> = Vec::with_capacity(quads.len());
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
    let id_sec = write_str_dict(&mut file, &id_vec, StrDictMode::Id)?;
    toc.push(TocEntry {
        kind: SectionKind::IdDict,
        section: id_sec,
        crc32_u32: 0,
    });
    // GNAME_DICT
    let gn_sec = write_str_dict(&mut file, &gn_vec, StrDictMode::GraphName)?;
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
                let compressed = zstd::encode_all(&raw.raw[..], 0)
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
            let raw_len = raw.raw.len() as u32;
            file.extend_from_slice(&raw_len.to_le_bytes());
            file.extend_from_slice(&raw.raw);
        }
        let sec = Section {
            off: start as u64,
            len: (file.len() - start) as u64,
        };
        // counts
        gid_rows.push((
            *id_id,
            *gn_id,
            sec,
            raw.n_t as u64,
            raw.n_s,
            raw.n_p,
            raw.n_t,
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
    write_gdir_rows(&mut file, &gid_rows);
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

    atomic_write(path.as_ref(), &file)
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
    id_map: HashMap<String, u32>,
    gn_map: HashMap<String, u32>,
    term_map: HashMap<Term, u64>,
    id_vec: Vec<String>,
    gn_vec: Vec<String>,
    term_vec: Vec<Term>,
    pending: Vec<EncodedQuint>,
    run_paths: Vec<PathBuf>,
    chunk_quads: usize,
    stats: StreamingWriteStats,
}

impl StreamingWriter {
    /// Create a streaming writer targeting `path` with `opts`.
    pub fn new<P: Into<PathBuf>>(path: P, opts: WriterOptions) -> Self {
        Self::with_options(path, StreamingWriterOptions::from_writer_options(opts))
    }

    /// Create a streaming writer targeting `path` with explicit streaming options.
    pub fn with_options<P: Into<PathBuf>>(path: P, opts: StreamingWriterOptions) -> Self {
        Self::with_options_and_hint(path, opts, 0)
    }

    /// Create a streaming writer with reserved capacity for approximately `n_quads` quads.
    pub fn with_capacity<P: Into<PathBuf>>(path: P, opts: WriterOptions, n_quads: usize) -> Self {
        Self::with_options_and_hint(
            path,
            StreamingWriterOptions::from_writer_options(opts),
            n_quads,
        )
    }

    /// Create a streaming writer with explicit options and an approximate workload hint.
    pub fn with_options_and_hint<P: Into<PathBuf>>(
        path: P,
        opts: StreamingWriterOptions,
        n_quads: usize,
    ) -> Self {
        let chunk_quads = resolve_chunk_quads(opts.spill_policy, n_quads);
        Self::build(path.into(), opts.writer, n_quads, chunk_quads)
    }

    /// Create a streaming writer with reserved capacity and a target in-memory chunk size.
    ///
    /// This remains available as an expert API. Prefer [`StreamingWriter::with_options`] or
    /// [`StreamingWriter::with_options_and_hint`] when callers want policy-driven configuration.
    pub fn with_chunk_capacity<P: Into<PathBuf>>(
        path: P,
        opts: WriterOptions,
        n_quads: usize,
        chunk_quads: usize,
    ) -> Self {
        Self::build(path.into(), opts, n_quads, chunk_quads.max(1))
    }

    fn build(path: PathBuf, opts: WriterOptions, n_quads: usize, chunk_quads: usize) -> Self {
        let estimated_unique_strings = n_quads.min(256);
        Self {
            opts,
            path,
            id_map: HashMap::with_capacity(estimated_unique_strings),
            gn_map: HashMap::with_capacity(estimated_unique_strings),
            term_map: HashMap::with_capacity(n_quads.saturating_mul(3)),
            id_vec: Vec::with_capacity(estimated_unique_strings),
            gn_vec: Vec::with_capacity(estimated_unique_strings),
            term_vec: Vec::with_capacity(n_quads.saturating_mul(3)),
            pending: Vec::with_capacity(chunk_quads.max(1)),
            run_paths: Vec::new(),
            chunk_quads: chunk_quads.max(1),
            stats: StreamingWriteStats {
                total_quads: 0,
                chunk_quads: chunk_quads.max(1),
                max_pending_quads: 0,
                run_count: 0,
                temp_bytes_written: 0,
            },
        }
    }

    fn intern_id_owned(&mut self, s: String) -> u32 {
        if let Some(&v) = self.id_map.get(s.as_str()) {
            return v;
        }
        let v = self.id_vec.len() as u32;
        self.id_vec.push(s.clone());
        self.id_map.insert(s, v);
        v
    }

    fn intern_gn_owned(&mut self, s: String) -> u32 {
        if let Some(&v) = self.gn_map.get(s.as_str()) {
            return v;
        }
        let v = self.gn_vec.len() as u32;
        self.gn_vec.push(s.clone());
        self.gn_map.insert(s, v);
        v
    }

    fn intern_term_owned(&mut self, t: Term) -> u64 {
        if let Some(&v) = self.term_map.get(&t) {
            return v;
        }
        let v = self.term_vec.len() as u64;
        self.term_vec.push(t.clone());
        self.term_map.insert(t, v);
        v
    }

    /// Add one 5‑tuple to the in‑memory builder.
    pub fn add(&mut self, q: Quint) -> Result<()> {
        let Quint { id, s, p, o, gname } = q;
        let id_id = self.intern_id_owned(id);
        let gn_id = self.intern_gn_owned(gname);
        let s = self.intern_term_owned(s);
        let p = self.intern_term_owned(p);
        let o = self.intern_term_owned(o);
        self.pending.push(EncodedQuint {
            id_id,
            gn_id,
            s,
            p,
            o,
        });
        self.stats.total_quads += 1;
        self.stats.max_pending_quads = self.stats.max_pending_quads.max(self.pending.len());
        if self.pending.len() >= self.chunk_quads {
            self.flush_run()?;
        }
        Ok(())
    }

    /// Finish building and write the file to disk.
    pub fn finalize(self) -> Result<()> {
        self.finalize_with_stats().map(|_| ())
    }

    /// Finish building, write the file to disk, and return streaming-build statistics.
    pub fn finalize_with_stats(mut self) -> Result<StreamingWriteStats> {
        // Build buffers using the same logic as write_file_with_options
        let mut file = vec![0u8; 32];
        let mut toc: Vec<TocEntry> = Vec::new();

        let id_sec = write_str_dict(&mut file, &self.id_vec, StrDictMode::Id)?;
        toc.push(TocEntry {
            kind: SectionKind::IdDict,
            section: id_sec,
            crc32_u32: 0,
        });
        let gn_sec = write_str_dict(&mut file, &self.gn_vec, StrDictMode::GraphName)?;
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
        self.write_sorted_triple_blocks(&mut file, &mut gid_rows)?;
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
        write_gdir_rows(&mut file, &gid_rows);
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

        // Durably publish the completed snapshot.
        atomic_write(&self.path, &file)?;
        Ok(self.stats)
    }

    fn flush_run(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.pending.sort_unstable();
        let path = temp_run_path(&self.path, self.run_paths.len());
        let file = fs::File::create(&path).map_err(R5Error::Io)?;
        let mut writer = BufWriter::new(file);
        for record in &self.pending {
            write_record(&mut writer, *record)?;
        }
        writer.flush().map_err(R5Error::Io)?;
        self.stats.run_count += 1;
        self.stats.temp_bytes_written += (self.pending.len() * 32) as u64;
        self.pending.clear();
        self.run_paths.push(path);
        Ok(())
    }

    fn write_sorted_triple_blocks(
        &mut self,
        file: &mut Vec<u8>,
        gid_rows: &mut Vec<GidRow>,
    ) -> Result<()> {
        let mut current_key: Option<GroupKey> = None;
        let mut current_spo: Vec<TripleIds> = Vec::new();

        if self.run_paths.is_empty() {
            self.pending.sort_unstable();
            for record in &self.pending {
                process_sorted_record(
                    *record,
                    &mut current_key,
                    &mut current_spo,
                    file,
                    gid_rows,
                    self.opts,
                )?;
            }
        } else {
            self.flush_run()?;
            let _cleanup = RunCleanup {
                paths: self.run_paths.clone(),
            };
            let mut readers: Vec<BufReader<fs::File>> = self
                .run_paths
                .iter()
                .map(|path| {
                    fs::File::open(path)
                        .map(BufReader::new)
                        .map_err(R5Error::Io)
                })
                .collect::<Result<_>>()?;
            let mut heap: BinaryHeap<Reverse<(EncodedQuint, usize)>> = BinaryHeap::new();
            for (idx, reader) in readers.iter_mut().enumerate() {
                if let Some(record) = read_record(reader)? {
                    heap.push(Reverse((record, idx)));
                }
            }
            while let Some(Reverse((record, idx))) = heap.pop() {
                process_sorted_record(
                    record,
                    &mut current_key,
                    &mut current_spo,
                    file,
                    gid_rows,
                    self.opts,
                )?;
                if let Some(next) = read_record(&mut readers[idx])? {
                    heap.push(Reverse((next, idx)));
                }
            }
        }

        if let Some((id_id, gn_id)) = current_key
            && !current_spo.is_empty()
        {
            write_group_block(file, gid_rows, self.opts, id_id, gn_id, &current_spo)?;
        }
        Ok(())
    }
}

impl StreamingWriterOptions {
    /// Create streaming options from batch-style writer options, using [`SpillPolicy::Auto`].
    pub fn from_writer_options(writer: WriterOptions) -> Self {
        Self {
            writer,
            spill_policy: SpillPolicy::Auto,
        }
    }
}

fn resolve_chunk_quads(policy: SpillPolicy, n_quads: usize) -> usize {
    const ENCODED_QUINT_BYTES: usize = 32;
    const MIN_CHUNK_QUADS: usize = 1_024;
    const DEFAULT_CHUNK_QUADS: usize = 65_536;
    const MAX_CHUNK_QUADS: usize = 262_144;

    match policy {
        SpillPolicy::Auto => {
            let hinted = if n_quads == 0 {
                DEFAULT_CHUNK_QUADS
            } else {
                n_quads.clamp(MIN_CHUNK_QUADS, DEFAULT_CHUNK_QUADS)
            };
            hinted.min(MAX_CHUNK_QUADS)
        }
        SpillPolicy::MaxPendingQuads(quads) => quads.max(1),
        SpillPolicy::TargetPendingBytes(bytes) => {
            let quads = bytes / ENCODED_QUINT_BYTES;
            quads.clamp(1, MAX_CHUNK_QUADS)
        }
    }
}

#[derive(Debug)]
struct RunCleanup {
    paths: Vec<PathBuf>,
}

impl Drop for RunCleanup {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

fn temp_run_path(base: &Path, run_idx: usize) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut path = base.to_path_buf();
    path.set_extension(format!("run.{run_idx}.{}.tmp", nanos));
    path
}

fn write_record(writer: &mut BufWriter<fs::File>, record: EncodedQuint) -> Result<()> {
    writer
        .write_all(&record.id_id.to_le_bytes())
        .map_err(R5Error::Io)?;
    writer
        .write_all(&record.gn_id.to_le_bytes())
        .map_err(R5Error::Io)?;
    writer
        .write_all(&record.s.to_le_bytes())
        .map_err(R5Error::Io)?;
    writer
        .write_all(&record.p.to_le_bytes())
        .map_err(R5Error::Io)?;
    writer
        .write_all(&record.o.to_le_bytes())
        .map_err(R5Error::Io)?;
    Ok(())
}

fn read_record(reader: &mut BufReader<fs::File>) -> Result<Option<EncodedQuint>> {
    let mut buf = [0u8; 32];
    let mut read = 0usize;
    while read < buf.len() {
        let n = reader.read(&mut buf[read..]).map_err(R5Error::Io)?;
        if n == 0 {
            if read == 0 {
                return Ok(None);
            }
            return Err(R5Error::Corrupt("truncated run record".into()));
        }
        read += n;
    }
    Ok(Some(EncodedQuint {
        id_id: u32::from_le_bytes(buf[0..4].try_into().unwrap()),
        gn_id: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        s: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
        p: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
        o: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
    }))
}

fn process_sorted_record(
    record: EncodedQuint,
    current_key: &mut Option<GroupKey>,
    current_spo: &mut Vec<TripleIds>,
    file: &mut Vec<u8>,
    gid_rows: &mut Vec<GidRow>,
    opts: WriterOptions,
) -> Result<()> {
    let key = (record.id_id, record.gn_id);
    if let Some((id_id, gn_id)) = *current_key
        && key != (id_id, gn_id)
    {
        write_group_block(file, gid_rows, opts, id_id, gn_id, current_spo)?;
        current_spo.clear();
    }
    if current_key.is_none() || current_key.as_ref() != Some(&key) {
        *current_key = Some(key);
    }
    current_spo.push((record.s, record.p, record.o));
    Ok(())
}

fn write_group_block(
    file: &mut Vec<u8>,
    gid_rows: &mut Vec<GidRow>,
    opts: WriterOptions,
    id_id: u32,
    gn_id: u32,
    spo: &[TripleIds],
) -> Result<()> {
    let start = file.len();
    let raw = build_raw_spo(spo)?;
    if opts.zstd {
        #[cfg(feature = "zstd")]
        {
            file.push(1u8);
            let compressed = zstd::encode_all(&raw.raw[..], 0)
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
        file.extend_from_slice(&(raw.raw.len() as u32).to_le_bytes());
        file.extend_from_slice(&raw.raw);
    }
    let sec = Section {
        off: start as u64,
        len: (file.len() - start) as u64,
    };
    gid_rows.push((id_id, gn_id, sec, raw.n_t as u64, raw.n_s, raw.n_p, raw.n_t));
    Ok(())
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
        use oxigraph::model::NamedOrBlankNodeRef;
        for t in graph.iter() {
            let s = match &t.subject {
                NamedOrBlankNodeRef::NamedNode(n) => Term::Iri(n.as_str().to_string()),
                NamedOrBlankNodeRef::BlankNode(b) => Term::BNode(format!("_:{}", b.as_str())),
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
    use oxigraph::model::{NamedNode, NamedOrBlankNodeRef, TermRef};
    let rdf_type = NamedNode::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type").ok()?;
    let owl_ontology = NamedNode::new("http://www.w3.org/2002/07/owl#Ontology").ok()?;
    for t in graph.iter() {
        if t.predicate == rdf_type.as_ref() && t.object == TermRef::NamedNode(owl_ontology.as_ref())
        {
            return Some(match t.subject {
                NamedOrBlankNodeRef::NamedNode(n) => n.as_str().to_string(),
                NamedOrBlankNodeRef::BlankNode(b) => format!("_:{}", b.as_str()),
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
            oxigraph::model::NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
            oxigraph::model::NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.as_str()),
        });
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrDictMode {
    Id,
    GraphName,
}

fn write_str_dict(buf: &mut Vec<u8>, strings: &[String], mode: StrDictMode) -> Result<Section> {
    let off = buf.len();
    // header 52 bytes
    buf.resize(buf.len() + 52, 0);
    let (blob_bytes, offs_bytes, front_coded) = build_best_str_dict_storage(strings, mode)?;
    let blob_off = buf.len();
    buf.extend_from_slice(&blob_bytes);
    let blob_len = blob_bytes.len();
    let offs_off = buf.len();
    buf.extend_from_slice(&offs_bytes);
    let mut offs_len = offs_bytes.len() as u64;
    if front_coded {
        offs_len |= STR_DICT_OFFS_FLAG_FRONT_CODED;
    }
    let (idx_bytes, grouped_idx) = build_best_dict_index(strings, mode);
    let idx_off;
    let mut idx_len;
    if !idx_bytes.is_empty() {
        idx_off = buf.len();
        buf.extend_from_slice(&idx_bytes);
        idx_len = idx_bytes.len() as u64;
        if grouped_idx {
            idx_len |= STR_DICT_IDX_FLAG_GROUPED;
        }
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
    buf[off + 28..off + 36].copy_from_slice(&offs_len.to_le_bytes());
    buf[off + 36..off + 44].copy_from_slice(&(idx_off as u64).to_le_bytes());
    buf[off + 44..off + 52].copy_from_slice(&idx_len.to_le_bytes());
    Ok(Section {
        off: off as u64,
        len: (buf.len() - off) as u64,
    })
}

fn build_best_str_dict_storage(
    strings: &[String],
    mode: StrDictMode,
) -> Result<(Vec<u8>, Vec<u8>, bool)> {
    let plain = build_plain_str_dict_storage(strings)?;
    if mode == StrDictMode::Id {
        return Ok((plain.0, plain.1, false));
    }

    let front = build_front_coded_str_dict_storage(strings)?;
    let plain_size = plain.0.len() + plain.1.len();
    let front_size = front.0.len() + front.1.len();
    if front_size < plain_size {
        Ok((front.0, front.1, true))
    } else {
        Ok((plain.0, plain.1, false))
    }
}

fn build_plain_str_dict_storage(strings: &[String]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut blob = Vec::new();
    let mut offs = Vec::with_capacity((strings.len() + 1) * 4);
    let mut cur = 0u32;
    for s in strings {
        offs.extend_from_slice(&cur.to_le_bytes());
        blob.extend_from_slice(s.as_bytes());
        cur = cur
            .checked_add(s.len() as u32)
            .ok_or_else(|| R5Error::Corrupt("blob size".into()))?;
    }
    offs.extend_from_slice(&cur.to_le_bytes());
    Ok((blob, offs))
}

fn build_front_coded_str_dict_storage(strings: &[String]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut blob = Vec::new();
    let mut restarts: Vec<u32> =
        Vec::with_capacity(strings.len().div_ceil(STR_DICT_FRONT_BLOCK_SIZE) + 1);
    let mut block_prev = "";
    for (idx, s) in strings.iter().enumerate() {
        if idx % STR_DICT_FRONT_BLOCK_SIZE == 0 {
            restarts.push(
                u32::try_from(blob.len())
                    .map_err(|_| R5Error::Corrupt("front-coded blob too large".into()))?,
            );
            push_uvarint(s.len() as u64, &mut blob);
            blob.extend_from_slice(s.as_bytes());
            block_prev = s;
        } else {
            let prefix = common_prefix_len(block_prev, s);
            let suffix = &s.as_bytes()[prefix..];
            push_uvarint(prefix as u64, &mut blob);
            push_uvarint(suffix.len() as u64, &mut blob);
            blob.extend_from_slice(suffix);
            block_prev = s;
        }
    }
    restarts.push(
        u32::try_from(blob.len())
            .map_err(|_| R5Error::Corrupt("front-coded blob too large".into()))?,
    );
    let mut offs = Vec::with_capacity(restarts.len() * 4);
    for restart in restarts {
        offs.extend_from_slice(&restart.to_le_bytes());
    }
    Ok((blob, offs))
}

fn build_best_dict_index(strings: &[String], mode: StrDictMode) -> (Vec<u8>, bool) {
    let mut entries: Vec<([u8; 16], u32)> = Vec::with_capacity(strings.len());
    for (i, s) in strings.iter().enumerate() {
        entries.push((dict_key16(s), i as u32));
    }
    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let flat = build_flat_dict_index(&entries);
    if mode == StrDictMode::Id {
        return (flat, false);
    }
    let grouped = build_grouped_dict_index(&entries);
    if !grouped.is_empty() && grouped.len() < flat.len() {
        (grouped, true)
    } else {
        (flat, false)
    }
}

fn build_flat_dict_index(entries: &[([u8; 16], u32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(entries.len() * 20);
    for (key, id) in entries {
        out.extend_from_slice(key);
        out.extend_from_slice(&id.to_le_bytes());
    }
    out
}

fn build_grouped_dict_index(entries: &[([u8; 16], u32)]) -> Vec<u8> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut groups: Vec<([u8; 16], Vec<u32>)> = Vec::new();
    for (key, id) in entries {
        match groups.last_mut() {
            Some((last_key, ids)) if last_key == key => ids.push(*id),
            _ => groups.push((*key, vec![*id])),
        }
    }
    let mut out = Vec::new();
    let n_groups = groups.len() as u32;
    out.extend_from_slice(&n_groups.to_le_bytes());
    let header_len = 4 + groups.len() * 24;
    let mut ids_off = header_len as u32;
    let mut ids_blob = Vec::with_capacity(entries.len() * 4);
    for (key, ids) in &groups {
        out.extend_from_slice(key);
        out.extend_from_slice(&ids_off.to_le_bytes());
        out.extend_from_slice(&(ids.len() as u32).to_le_bytes());
        for id in ids {
            ids_blob.extend_from_slice(&id.to_le_bytes());
        }
        ids_off += (ids.len() * 4) as u32;
    }
    out.extend_from_slice(&ids_blob);
    out
}

fn dict_key16(s: &str) -> [u8; 16] {
    let mut key = [0u8; 16];
    for (dst, src) in key.iter_mut().zip(s.bytes()) {
        *dst = src.to_ascii_lowercase();
    }
    key
}

fn intern_component(
    map: &mut HashMap<String, u32>,
    vec: &mut Vec<String>,
    value: &str,
) -> Result<u32> {
    if let Some(&id) = map.get(value) {
        return Ok(id);
    }
    let id = u32::try_from(vec.len()).map_err(|_| R5Error::Invalid("component dict too large"))?;
    let owned = value.to_string();
    vec.push(owned.clone());
    map.insert(owned, id);
    Ok(id)
}

fn component_dict_size(strings: &[String]) -> Result<u64> {
    let blob_len = strings.iter().try_fold(0u64, |acc, s| {
        acc.checked_add(s.len() as u64)
            .ok_or_else(|| R5Error::Corrupt("component blob overflow".into()))
    })?;
    let offs_len = (u64::try_from(strings.len())
        .map_err(|_| R5Error::Invalid("component dict too large"))?
        + 1)
    .checked_mul(4)
    .ok_or_else(|| R5Error::Corrupt("component offsets overflow".into()))?;
    20u64
        .checked_add(blob_len)
        .and_then(|v| v.checked_add(offs_len))
        .ok_or_else(|| R5Error::Corrupt("component dict size overflow".into()))
}

fn write_component_dict(buf: &mut Vec<u8>, strings: &[String]) -> Result<()> {
    let n =
        u32::try_from(strings.len()).map_err(|_| R5Error::Invalid("component dict too large"))?;
    let blob_len = strings.iter().try_fold(0u32, |acc, s| {
        acc.checked_add(s.len() as u32)
            .ok_or_else(|| R5Error::Corrupt("component blob overflow".into()))
    })?;
    let offs_len = (u64::from(n) + 1)
        .checked_mul(4)
        .ok_or_else(|| R5Error::Corrupt("component offsets overflow".into()))?;

    buf.extend_from_slice(&n.to_le_bytes());
    buf.extend_from_slice(&(u64::from(blob_len)).to_le_bytes());
    buf.extend_from_slice(&offs_len.to_le_bytes());
    for s in strings {
        buf.extend_from_slice(s.as_bytes());
    }
    let mut cur = 0u32;
    for s in strings {
        buf.extend_from_slice(&cur.to_le_bytes());
        cur = cur
            .checked_add(s.len() as u32)
            .ok_or_else(|| R5Error::Corrupt("component blob overflow".into()))?;
    }
    buf.extend_from_slice(&cur.to_le_bytes());
    Ok(())
}

fn build_inline_term_data(terms: &[Term]) -> Result<TermDataBuild> {
    let mut kinds = Vec::with_capacity(terms.len());
    let mut data = Vec::new();
    let mut offs = Vec::with_capacity(terms.len() + 1);
    let mut cur = 0u64;
    offs.push(cur);
    for t in terms {
        match t {
            Term::Iri(s) => {
                kinds.push(TERM_KIND_IRI);
                data.extend_from_slice(s.as_bytes());
                cur += s.len() as u64;
            }
            Term::BNode(s) => {
                kinds.push(TERM_KIND_BNODE);
                data.extend_from_slice(s.as_bytes());
                cur += s.len() as u64;
            }
            Term::Literal { lex, dt, lang } => {
                kinds.push(TERM_KIND_LITERAL_INLINE);
                push_uvarint(lex.len() as u64, &mut data);
                data.extend_from_slice(lex.as_bytes());
                match dt {
                    Some(d) => {
                        data.push(1);
                        push_uvarint(d.len() as u64, &mut data);
                        data.extend_from_slice(d.as_bytes());
                    }
                    None => data.push(0),
                }
                match lang {
                    Some(l) => {
                        data.push(1);
                        push_uvarint(l.len() as u64, &mut data);
                        data.extend_from_slice(l.as_bytes());
                    }
                    None => data.push(0),
                }
                cur = data.len() as u64;
            }
        }
        offs.push(cur);
    }
    Ok(TermDataBuild {
        kinds,
        data,
        offs,
        literal_components: false,
    })
}

fn build_component_term_data(terms: &[Term]) -> Result<Option<TermDataBuild>> {
    let mut pool = LiteralComponentPool::default();
    let mut inline_literal_bytes = 0usize;
    let mut component_literal_bytes = 0usize;
    let mut literal_count = 0usize;

    for term in terms {
        if let Term::Literal { lex, dt, lang } = term {
            literal_count += 1;
            inline_literal_bytes += uvarint_len(lex.len() as u64) + lex.len() + 2;
            let lex_id = intern_component(&mut pool.lex_map, &mut pool.lex, lex)?;
            component_literal_bytes += uvarint_len(lex_id as u64);
            if let Some(dt) = dt {
                inline_literal_bytes += uvarint_len(dt.len() as u64) + dt.len();
                let dt_id = intern_component(&mut pool.dt_map, &mut pool.dt, dt)?;
                component_literal_bytes += uvarint_len(dt_id as u64 + 1);
            } else {
                component_literal_bytes += 1;
            }
            if let Some(lang) = lang {
                inline_literal_bytes += uvarint_len(lang.len() as u64) + lang.len();
                let lang_id = intern_component(&mut pool.lang_map, &mut pool.lang, lang)?;
                component_literal_bytes += uvarint_len(lang_id as u64 + 1);
            } else {
                component_literal_bytes += 1;
            }
        }
    }

    if literal_count == 0 {
        return Ok(None);
    }

    let dict_overhead = component_dict_size(&pool.lex)?
        .checked_add(component_dict_size(&pool.dt)?)
        .ok_or_else(|| R5Error::Corrupt("component dict size overflow".into()))?
        .checked_add(component_dict_size(&pool.lang)?)
        .ok_or_else(|| R5Error::Corrupt("component dict size overflow".into()))?;

    if dict_overhead + component_literal_bytes as u64 >= inline_literal_bytes as u64 {
        return Ok(None);
    }

    let mut data = Vec::new();
    write_component_dict(&mut data, &pool.lex)?;
    write_component_dict(&mut data, &pool.dt)?;
    write_component_dict(&mut data, &pool.lang)?;

    let mut kinds = Vec::with_capacity(terms.len());
    let mut offs = Vec::with_capacity(terms.len() + 1);
    let mut cur = data.len() as u64;
    offs.push(cur);
    for term in terms {
        match term {
            Term::Iri(s) => {
                kinds.push(TERM_KIND_IRI);
                data.extend_from_slice(s.as_bytes());
            }
            Term::BNode(s) => {
                kinds.push(TERM_KIND_BNODE);
                data.extend_from_slice(s.as_bytes());
            }
            Term::Literal { lex, dt, lang } => {
                kinds.push(TERM_KIND_LITERAL_COMPONENTS);
                let lex_id = pool.lex_map[lex] as u64;
                push_uvarint(lex_id, &mut data);
                push_uvarint(
                    dt.as_ref()
                        .map(|value| u64::from(pool.dt_map[value]) + 1)
                        .unwrap_or(0),
                    &mut data,
                );
                push_uvarint(
                    lang.as_ref()
                        .map(|value| u64::from(pool.lang_map[value]) + 1)
                        .unwrap_or(0),
                    &mut data,
                );
            }
        }
        cur = data.len() as u64;
        offs.push(cur);
    }

    Ok(Some(TermDataBuild {
        kinds,
        data,
        offs,
        literal_components: true,
    }))
}

fn write_term_dict(buf: &mut Vec<u8>, terms: &[Term]) -> Result<Section> {
    write_term_dict_with_width(buf, terms, None)
}

fn write_term_dict_with_width(
    buf: &mut Vec<u8>,
    terms: &[Term],
    force_width: Option<u8>,
) -> Result<Section> {
    let off = buf.len();
    // header 33 bytes
    buf.resize(buf.len() + 33, 0);
    let inline = build_inline_term_data(terms)?;
    let term_data = match build_component_term_data(terms)? {
        Some(component) if component.data.len() < inline.data.len() => component,
        _ => inline,
    };

    let kinds_off = buf.len();
    buf.extend_from_slice(&term_data.kinds);
    let data_off = buf.len();
    buf.extend_from_slice(&term_data.data);
    let data_len = term_data
        .offs
        .last()
        .copied()
        .unwrap_or(term_data.data.len() as u64);
    let width = force_width.unwrap_or(if data_len <= u32::MAX as u64 { 4 } else { 8 });
    if width != 4 && width != 8 {
        return Err(R5Error::Invalid("unsupported term dict width"));
    }
    // offs u32*(n+1) or u64*(n+1)
    let offs_off = buf.len();
    for o in term_data.offs {
        if width == 4 {
            let o = u32::try_from(o).map_err(|_| R5Error::Invalid("term dict width overflow"))?;
            buf.extend_from_slice(&o.to_le_bytes());
        } else {
            buf.extend_from_slice(&o.to_le_bytes());
        }
    }

    // fill header
    buf[off] = (width & TERM_DICT_WIDTH_MASK)
        | if term_data.literal_components {
            TERM_DICT_FLAG_LITERAL_COMPONENTS
        } else {
            0
        };
    buf[off + 1..off + 9].copy_from_slice(&(terms.len() as u64).to_le_bytes());
    buf[off + 9..off + 17].copy_from_slice(&(kinds_off as u64).to_le_bytes());
    buf[off + 17..off + 25].copy_from_slice(&(data_off as u64).to_le_bytes());
    buf[off + 25..off + 33].copy_from_slice(&(offs_off as u64).to_le_bytes());
    Ok(Section {
        off: off as u64,
        len: (buf.len() - off) as u64,
    })
}

/// Encode sorted SPO triples into the compact CSR-like binary format.
///
/// Given triples sorted by (S, P, O), this builds a two-level compressed
/// structure:
///   - **S_vals / S_heads**: unique subjects, with each S_head pointing into P_vals
///   - **P_vals / P_heads**: unique predicates per subject, with each P_head
///     pointing into O_vals
///   - **O_vals**: object term-ids for each (S, P) group
///
/// All value arrays are delta-coded (first value stored literally, subsequent
/// values as differences from the predecessor). Counts and values are encoded
/// as LEB128 uvarints.
fn build_raw_spo(spo: &[(u64, u64, u64)]) -> Result<RawSpoBuild> {
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
    Ok(RawSpoBuild {
        raw: out,
        n_s: s_vals.len() as u32,
        n_p: p_vals.len() as u32,
        n_t: o_vals.len() as u32,
    })
}

fn write_postings_index(buf: &mut Vec<u8>, lists: &[Vec<u64>]) -> Result<Section> {
    let off = buf.len();
    buf.resize(buf.len() + 24, 0); // header
    let offs_off = buf.len();
    buf.extend_from_slice(&0u64.to_le_bytes()); // first offset is always 0
    let mut blob = Vec::new();
    for list in lists {
        if list.is_empty() {
            push_uvarint(0, &mut blob);
        } else {
            push_uvarint(list.len() as u64, &mut blob);
            push_uvarint(list[0], &mut blob);
            for w in list.windows(2) {
                push_uvarint(w[1] - w[0], &mut blob);
            }
        }
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
    let compact = pairs.iter().all(|(_, _, gid)| u32::try_from(*gid).is_ok());
    for (id_id, gn_id, gid) in pairs {
        buf.extend_from_slice(&id_id.to_le_bytes());
        buf.extend_from_slice(&gn_id.to_le_bytes());
        if compact {
            buf.extend_from_slice(&(*gid as u32).to_le_bytes());
        } else {
            buf.extend_from_slice(&gid.to_le_bytes());
        }
    }
    Ok(Section {
        off: off as u64,
        len: (buf.len() - off) as u64,
    })
}

#[cfg(test)]
#[allow(clippy::io_other_error)] // Kept explicit for readable failure-injection tests.
mod atomic_write_tests {
    use super::*;

    #[test]
    fn failure_before_replace_preserves_previous_file() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("catalog.r5tu");
        fs::write(&destination, b"previous-valid-snapshot").unwrap();

        let result = atomic_write_before_replace(&destination, b"replacement", || {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "injected pre-replace failure",
            ))
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"previous-valid-snapshot");
        let leftovers: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
