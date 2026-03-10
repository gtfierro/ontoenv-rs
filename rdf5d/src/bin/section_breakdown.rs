use std::error::Error;
use std::fs;
use std::path::PathBuf;

use clap::Parser;
use rdf5d::{R5tuFile, header::SectionKind};

const STR_DICT_OFFS_FLAG_FRONT_CODED: u64 = 1 << 63;
const STR_DICT_IDX_FLAG_GROUPED: u64 = 1 << 63;

#[derive(Debug, Parser)]
#[command(name = "section_breakdown")]
#[command(about = "Report rdf5d section sizes and key internal subcomponents")]
struct Args {
    #[arg(long)]
    file: PathBuf,
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Component {
    name: &'static str,
    bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SectionReport {
    kind: SectionKind,
    bytes: u64,
    components: Vec<Component>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BreakdownReport {
    file_bytes: u64,
    header_bytes: u64,
    toc_bytes: u64,
    trailer_bytes: u64,
    sections: Vec<SectionReport>,
}

fn kind_name(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::TermDict => "TERM_DICT",
        SectionKind::IdDict => "ID_DICT",
        SectionKind::GNameDict => "GNAME_DICT",
        SectionKind::GDir => "GDIR",
        SectionKind::IdxId2Gid => "IDX_ID2GID",
        SectionKind::IdxGName2Gid => "IDX_GNAME2GID",
        SectionKind::IdxPair2Gid => "IDX_PAIR2GID",
        SectionKind::TripleBlocks => "TRIPLE_BLOCKS",
    }
}

fn read_u32(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap())
}

fn parse_str_dict(bytes: &[u8], sec_off: usize, sec_len: u64, kind: SectionKind) -> SectionReport {
    let blob_len = read_u64(bytes, sec_off + 12);
    let offs_len_raw = read_u64(bytes, sec_off + 28);
    let offs_len = offs_len_raw & !STR_DICT_OFFS_FLAG_FRONT_CODED;
    let idx_len_raw = read_u64(bytes, sec_off + 44);
    let idx_len = idx_len_raw & !STR_DICT_IDX_FLAG_GROUPED;
    let header_len = 52u64;
    SectionReport {
        kind,
        bytes: sec_len,
        components: vec![
            Component {
                name: "header",
                bytes: header_len,
            },
            Component {
                name: "blob",
                bytes: blob_len,
            },
            Component {
                name: "offsets",
                bytes: offs_len,
            },
            Component {
                name: "front_coded",
                bytes: u64::from((offs_len_raw & STR_DICT_OFFS_FLAG_FRONT_CODED) != 0),
            },
            Component {
                name: "coarse_index",
                bytes: idx_len,
            },
            Component {
                name: "grouped_index",
                bytes: u64::from((idx_len_raw & STR_DICT_IDX_FLAG_GROUPED) != 0),
            },
        ],
    }
}

fn parse_term_dict(bytes: &[u8], sec_off: usize, sec_len: u64) -> SectionReport {
    let n_terms = read_u64(bytes, sec_off + 1);
    let width = bytes[sec_off] as u64;
    let kinds_off = read_u64(bytes, sec_off + 9) as usize;
    let data_off = read_u64(bytes, sec_off + 17) as usize;
    let offs_off = read_u64(bytes, sec_off + 25) as usize;
    let header_len = 33u64;
    let kinds_len = data_off as u64 - kinds_off as u64;
    let offsets_len = (n_terms + 1) * width;
    let data_len = offs_off as u64 - data_off as u64;
    SectionReport {
        kind: SectionKind::TermDict,
        bytes: sec_len,
        components: vec![
            Component {
                name: "header",
                bytes: header_len,
            },
            Component {
                name: "kinds",
                bytes: kinds_len,
            },
            Component {
                name: "offsets",
                bytes: offsets_len,
            },
            Component {
                name: "data",
                bytes: data_len,
            },
        ],
    }
}

fn parse_gdir(bytes: &[u8], sec_off: usize, sec_len: u64) -> SectionReport {
    let n_rows = read_u64(bytes, sec_off);
    let row_size = read_u32(bytes, sec_off + 8) as u64;
    SectionReport {
        kind: SectionKind::GDir,
        bytes: sec_len,
        components: vec![
            Component {
                name: "header",
                bytes: 16,
            },
            Component {
                name: "rows",
                bytes: n_rows * row_size,
            },
        ],
    }
}

fn parse_postings(bytes: &[u8], sec_off: usize, sec_len: u64, kind: SectionKind) -> SectionReport {
    let n_keys = read_u64(bytes, sec_off);
    let blob_off = read_u64(bytes, sec_off + 16) as usize;
    let offsets_len = n_keys.saturating_add(1) * 8;
    let blob_len = sec_len.saturating_sub((blob_off - sec_off) as u64);
    SectionReport {
        kind,
        bytes: sec_len,
        components: vec![
            Component {
                name: "header",
                bytes: 24,
            },
            Component {
                name: "offsets",
                bytes: offsets_len,
            },
            Component {
                name: "postings_blob",
                bytes: blob_len,
            },
        ],
    }
}

fn parse_pair_index(bytes: &[u8], sec_off: usize, sec_len: u64) -> SectionReport {
    let n_pairs = read_u64(bytes, sec_off);
    let pairs_off = read_u64(bytes, sec_off + 8) as usize;
    let payload_len = sec_len.saturating_sub((pairs_off - sec_off) as u64);
    let entry_size = if n_pairs == 0 {
        0
    } else {
        payload_len / n_pairs
    };
    SectionReport {
        kind: SectionKind::IdxPair2Gid,
        bytes: sec_len,
        components: vec![
            Component {
                name: "header",
                bytes: 16,
            },
            Component {
                name: "pair_entries",
                bytes: payload_len,
            },
            Component {
                name: "pair_entry_size_bytes",
                bytes: entry_size,
            },
        ],
    }
}

fn parse_triple_blocks(bytes: &[u8], triple_len: u64, gdir_off: usize) -> SectionReport {
    let n_rows = read_u64(bytes, gdir_off) as usize;
    let row_size = read_u32(bytes, gdir_off + 8) as usize;
    let mut block_headers = 0u64;
    let mut raw_payload = 0u64;
    let mut zstd_payload = 0u64;

    for row in 0..n_rows {
        let off = gdir_off + 16 + row * row_size;
        let triples_off = if row_size == 32 {
            read_u32(bytes, off + 8) as u64
        } else {
            read_u64(bytes, off + 8)
        } as usize;
        let triples_len = if row_size == 32 {
            read_u32(bytes, off + 12) as u64
        } else {
            read_u64(bytes, off + 16)
        };
        if triples_len < 5 {
            continue;
        }
        block_headers += 5;
        let payload_len = triples_len - 5;
        match bytes[triples_off] {
            0 => raw_payload += payload_len,
            1 => zstd_payload += payload_len,
            _ => {}
        }
    }

    SectionReport {
        kind: SectionKind::TripleBlocks,
        bytes: triple_len,
        components: vec![
            Component {
                name: "block_headers",
                bytes: block_headers,
            },
            Component {
                name: "raw_payload",
                bytes: raw_payload,
            },
            Component {
                name: "zstd_payload",
                bytes: zstd_payload,
            },
        ],
    }
}

fn analyze_file(path: &PathBuf) -> Result<BreakdownReport, Box<dyn Error>> {
    let file = R5tuFile::open(path)?;
    let bytes = fs::read(path)?;
    let file_bytes = bytes.len() as u64;
    let header_bytes = 32u64;
    let toc_bytes = file.header().toc_len_u32 as u64 * 32;
    let mut sections = Vec::with_capacity(file.toc().len());

    let gdir_off = file
        .section(SectionKind::GDir)
        .expect("validated file has GDIR")
        .off as usize;

    for entry in file.toc() {
        let sec_off = entry.section.off as usize;
        let sec_len = entry.section.len;
        let report = match entry.kind {
            SectionKind::IdDict => parse_str_dict(&bytes, sec_off, sec_len, SectionKind::IdDict),
            SectionKind::GNameDict => {
                parse_str_dict(&bytes, sec_off, sec_len, SectionKind::GNameDict)
            }
            SectionKind::TermDict => parse_term_dict(&bytes, sec_off, sec_len),
            SectionKind::GDir => parse_gdir(&bytes, sec_off, sec_len),
            SectionKind::IdxId2Gid => {
                parse_postings(&bytes, sec_off, sec_len, SectionKind::IdxId2Gid)
            }
            SectionKind::IdxGName2Gid => {
                parse_postings(&bytes, sec_off, sec_len, SectionKind::IdxGName2Gid)
            }
            SectionKind::IdxPair2Gid => parse_pair_index(&bytes, sec_off, sec_len),
            SectionKind::TripleBlocks => parse_triple_blocks(&bytes, sec_len, gdir_off),
        };
        sections.push(report);
    }

    let section_bytes = sections.iter().map(|section| section.bytes).sum::<u64>();
    let trailer_bytes = file_bytes.saturating_sub(header_bytes + toc_bytes + section_bytes);
    Ok(BreakdownReport {
        file_bytes,
        header_bytes,
        toc_bytes,
        trailer_bytes,
        sections,
    })
}

fn print_human(report: &BreakdownReport) {
    println!("file_bytes={}", report.file_bytes);
    println!(
        "header={} toc={} trailer={}",
        report.header_bytes, report.toc_bytes, report.trailer_bytes
    );
    for section in &report.sections {
        let pct = (section.bytes as f64 / report.file_bytes as f64) * 100.0;
        println!(
            "{} bytes={} pct={:.2}",
            kind_name(section.kind),
            section.bytes,
            pct
        );
        for component in &section.components {
            let pct = (component.bytes as f64 / section.bytes as f64) * 100.0;
            println!(
                "  {} bytes={} pct_of_section={:.2}",
                component.name, component.bytes, pct
            );
        }
    }
}

fn print_json(report: &BreakdownReport) {
    println!("{{");
    println!("  \"file_bytes\": {},", report.file_bytes);
    println!("  \"header_bytes\": {},", report.header_bytes);
    println!("  \"toc_bytes\": {},", report.toc_bytes);
    println!("  \"trailer_bytes\": {},", report.trailer_bytes);
    println!("  \"sections\": [");
    for (section_idx, section) in report.sections.iter().enumerate() {
        println!("    {{");
        println!("      \"kind\": \"{}\",", kind_name(section.kind));
        println!("      \"bytes\": {},", section.bytes);
        println!("      \"components\": [");
        for (component_idx, component) in section.components.iter().enumerate() {
            let suffix = if component_idx + 1 == section.components.len() {
                ""
            } else {
                ","
            };
            println!(
                "        {{\"name\": \"{}\", \"bytes\": {}}}{}",
                component.name, component.bytes, suffix
            );
        }
        let suffix = if section_idx + 1 == report.sections.len() {
            ""
        } else {
            ","
        };
        println!("      ]");
        println!("    }}{}", suffix);
    }
    println!("  ]");
    println!("}}");
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let report = analyze_file(&args.file)?;
    if args.json {
        print_json(&report);
    } else {
        print_human(&report);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{analyze_file, kind_name};
    use rdf5d::{Quint, header::SectionKind, write_file};

    #[test]
    fn breakdown_accounts_for_entire_file() {
        let mut path = std::env::temp_dir();
        path.push("section_breakdown_test.r5tu");
        let quads = vec![Quint {
            id: "dataset/a".into(),
            s: rdf5d::Term::Iri("http://ex/s".into()),
            p: rdf5d::Term::Iri("http://ex/p".into()),
            o: rdf5d::Term::Iri("http://ex/o".into()),
            gname: "http://example.org/g".into(),
        }];
        write_file(&path, &quads).unwrap();

        let report = analyze_file(&path).unwrap();
        let section_bytes = report
            .sections
            .iter()
            .map(|section| section.bytes)
            .sum::<u64>();
        assert_eq!(
            report.file_bytes,
            report.header_bytes + report.toc_bytes + report.trailer_bytes + section_bytes
        );
        assert!(
            report
                .sections
                .iter()
                .any(|section| section.kind == SectionKind::TermDict)
        );
        assert_eq!(kind_name(SectionKind::IdxPair2Gid), "IDX_PAIR2GID");

        let _ = std::fs::remove_file(&path);
    }
}
