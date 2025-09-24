//! Header, TOC, and section kinds for R5TU files (ARCH.md §1).

/// Enumerates the kinds of sections in an R5TU file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum SectionKind {
    TermDict = 1,
    IdDict = 2,
    GNameDict = 3,
    GDir = 4,
    IdxId2Gid = 5,
    IdxGName2Gid = 6,
    IdxPair2Gid = 7,
    TripleBlocks = 8,
}

impl SectionKind {
    /// Convert a little‑endian `u16` value into a kind, if recognized.
    pub fn from_u16(v: u16) -> Option<Self> {
        use SectionKind::*;
        Some(match v {
            1 => TermDict,
            2 => IdDict,
            3 => GNameDict,
            4 => GDir,
            5 => IdxId2Gid,
            6 => IdxGName2Gid,
            7 => IdxPair2Gid,
            8 => TripleBlocks,
            _ => return None,
        })
    }
}

/// Byte span for a section.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub off: u64,
    pub len: u64,
}

/// Entry in the table of contents mapping a kind to its section.
#[derive(Debug, Clone, Copy)]
pub struct TocEntry {
    pub kind: SectionKind,
    pub section: Section,
    pub crc32_u32: u32, // 0 if absent
}

/// Parsed fixed‑size file header.
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub magic: [u8; 4],
    pub version_u16: u16,
    pub flags_u16: u16,
    pub created_unix64: u64,
    pub toc_off_u64: u64,
    pub toc_len_u32: u32,
    pub reserved_u32: u32,
}

impl Header {
    /// Parse a header from the first 32 bytes of `buf`.
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 32 {
            return None;
        }
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&buf[0..4]);
        let version_u16 = u16::from_le_bytes([buf[4], buf[5]]);
        let flags_u16 = u16::from_le_bytes([buf[6], buf[7]]);
        let created_unix64 = u64::from_le_bytes([
            buf[8], buf[9], buf[10], buf[11], buf[12], buf[13], buf[14], buf[15],
        ]);
        let toc_off_u64 = u64::from_le_bytes([
            buf[16], buf[17], buf[18], buf[19], buf[20], buf[21], buf[22], buf[23],
        ]);
        let toc_len_u32 = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]);
        let reserved_u32 = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]);
        Some(Header {
            magic,
            version_u16,
            flags_u16,
            created_unix64,
            toc_off_u64,
            toc_len_u32,
            reserved_u32,
        })
    }
}

/// Parse the TOC entries referenced by `hdr`.
pub fn parse_toc(buf: &[u8], hdr: &Header) -> Option<Vec<TocEntry>> {
    // Each entry is 32 bytes; TOC starts at hdr.toc_off_u64
    let toc_off = hdr.toc_off_u64 as usize;
    let n = hdr.toc_len_u32 as usize;
    let need = toc_off.checked_add(n.checked_mul(32)?)?;
    if need > buf.len() {
        return None;
    }

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let off = toc_off + i * 32;
        let kind_u16 = u16::from_le_bytes([buf[off], buf[off + 1]]);
        let kind = SectionKind::from_u16(kind_u16)?;
        // skip reserved_u16
        let off_u64 = u64::from_le_bytes([
            buf[off + 4],
            buf[off + 5],
            buf[off + 6],
            buf[off + 7],
            buf[off + 8],
            buf[off + 9],
            buf[off + 10],
            buf[off + 11],
        ]);
        let len_u64 = u64::from_le_bytes([
            buf[off + 12],
            buf[off + 13],
            buf[off + 14],
            buf[off + 15],
            buf[off + 16],
            buf[off + 17],
            buf[off + 18],
            buf[off + 19],
        ]);

        // crc32_u32 at off+20..24 (optional), reserved_u32 off+24..28 (ignored here)
        let crc32_u32 =
            u32::from_le_bytes([buf[off + 20], buf[off + 21], buf[off + 22], buf[off + 23]]);
        out.push(TocEntry {
            kind,
            section: Section {
                off: off_u64,
                len: len_u64,
            },
            crc32_u32,
        });
    }
    Some(out)
}

/// True if `section` lies entirely within a buffer of `buf_len` bytes.
pub fn section_in_bounds(buf_len: usize, section: Section) -> bool {
    let start = section.off as usize;
    let len = section.len as usize;
    start <= buf_len && start.saturating_add(len) <= buf_len
}

/// Compute IEEE CRC‑32.
pub fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        let mut x = (crc ^ (b as u32)) & 0xFF;
        for _ in 0..8 {
            let lsb = x & 1;
            x >>= 1;
            if lsb != 0 {
                x ^= 0xEDB88320;
            }
        }
        crc = (crc >> 8) ^ x;
    }
    crc ^ 0xFFFF_FFFF
}

/// Parse the optional 16‑byte footer containing the global CRC and magic.
pub fn parse_footer(buf: &[u8]) -> Option<(u32, [u8; 12])> {
    if buf.len() < 16 {
        return None;
    }
    let base = buf.len() - 16;
    let mut magic = [0u8; 12];
    magic.copy_from_slice(&buf[base + 4..base + 16]);
    if &magic != b"R5TU_ENDMARK" {
        return None;
    }
    let crc = u32::from_le_bytes([buf[base], buf[base + 1], buf[base + 2], buf[base + 3]]);
    Some((crc, magic))
}
