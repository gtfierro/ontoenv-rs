use std::error::Error;
use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, ValueEnum};
use rdf5d::header::SectionKind;
use rdf5d::{
    IntegrityMode, OpenOptions, Quint, R5tuFile, Term, WriterOptions, write_file_with_options,
};

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum Workload {
    ManySmallGraphs,
    OneLargeGraph,
    RepeatedLiterals,
    HighCardinalityNames,
    All,
}

#[derive(Debug, Parser)]
#[command(name = "workload_profile")]
#[command(about = "Profile representative rdf5d workloads and report runtime plus section sizes")]
struct Args {
    #[arg(long, value_enum, default_value_t = Workload::All)]
    workload: Workload,
    #[arg(long, default_value_t = 5)]
    iterations: usize,
    #[arg(long, default_value_t = false)]
    zstd: bool,
    #[arg(long = "with-crc", default_value_t = true)]
    with_crc: bool,
    #[arg(long)]
    output_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
enum WorkloadKind {
    Balanced,
    RepeatedLiterals,
    HighCardinalityNames,
}

#[derive(Clone, Copy, Debug)]
struct WorkloadCase {
    name: &'static str,
    kind: WorkloadKind,
    n_graphs: usize,
    triples_per_graph: usize,
}

impl WorkloadCase {
    fn total_quads(self) -> usize {
        self.n_graphs * self.triples_per_graph
    }
}

fn workload_cases(requested: Workload) -> Vec<WorkloadCase> {
    let all = [
        WorkloadCase {
            name: "many_small_graphs",
            kind: WorkloadKind::Balanced,
            n_graphs: 200,
            triples_per_graph: 5,
        },
        WorkloadCase {
            name: "one_large_graph",
            kind: WorkloadKind::Balanced,
            n_graphs: 1,
            triples_per_graph: 20_000,
        },
        WorkloadCase {
            name: "repeated_literals",
            kind: WorkloadKind::RepeatedLiterals,
            n_graphs: 25,
            triples_per_graph: 400,
        },
        WorkloadCase {
            name: "high_cardinality_names",
            kind: WorkloadKind::HighCardinalityNames,
            n_graphs: 200,
            triples_per_graph: 5,
        },
    ];
    match requested {
        Workload::All => all.to_vec(),
        Workload::ManySmallGraphs => vec![all[0]],
        Workload::OneLargeGraph => vec![all[1]],
        Workload::RepeatedLiterals => vec![all[2]],
        Workload::HighCardinalityNames => vec![all[3]],
    }
}

fn dataset_id(kind: WorkloadKind, g: usize) -> String {
    match kind {
        WorkloadKind::Balanced => format!("dataset/{g}"),
        WorkloadKind::RepeatedLiterals => format!("dataset/repeated/{g}"),
        WorkloadKind::HighCardinalityNames => format!(
            "urn:dataset:very:long:prefix:{g:04}:{}:{}",
            g % 17,
            10_000 + g
        ),
    }
}

fn graph_name(kind: WorkloadKind, g: usize) -> String {
    match kind {
        WorkloadKind::Balanced => format!("http://example.org/graph/{g}"),
        WorkloadKind::RepeatedLiterals => format!("http://example.org/graph/repeated/{g}"),
        WorkloadKind::HighCardinalityNames => format!(
            "https://graphs.example.org/ns/really/long/graph/name/{g:04}/{}-{}",
            g % 23,
            1_000 + g
        ),
    }
}

fn generate_quints(case: WorkloadCase) -> Vec<Quint> {
    match case.kind {
        WorkloadKind::Balanced => {
            let mut quints = Vec::with_capacity(case.total_quads());
            for g in 0..case.n_graphs {
                let id = dataset_id(case.kind, g);
                let gname = graph_name(case.kind, g);
                for t in 0..case.triples_per_graph {
                    let s = if t % 5 == 0 {
                        Term::BNode(format!("b{g}_{t}"))
                    } else {
                        Term::Iri(format!("http://example.org/s/{g}/{t}"))
                    };
                    let p = Term::Iri(format!("http://example.org/p/{}", t % 20));
                    let o = match t % 4 {
                        0 => Term::Iri(format!("http://example.org/o/{t}")),
                        1 => Term::Literal {
                            lex: format!("value {t}"),
                            dt: None,
                            lang: None,
                        },
                        2 => Term::Literal {
                            lex: format!("typed {t}"),
                            dt: Some("http://www.w3.org/2001/XMLSchema#string".into()),
                            lang: None,
                        },
                        _ => Term::Literal {
                            lex: format!("hello {t}"),
                            dt: None,
                            lang: Some("en".into()),
                        },
                    };
                    quints.push(Quint {
                        id: id.clone(),
                        s,
                        p,
                        o,
                        gname: gname.clone(),
                    });
                }
            }
            quints
        }
        WorkloadKind::RepeatedLiterals => {
            let mut quints = Vec::with_capacity(case.total_quads());
            let lex_pool = [
                "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
            ];
            let dt_pool = [
                "http://www.w3.org/2001/XMLSchema#string",
                "http://www.w3.org/2001/XMLSchema#date",
                "http://www.w3.org/2001/XMLSchema#integer",
            ];
            let lang_pool = ["en", "fr", "de"];
            for g in 0..case.n_graphs {
                let id = dataset_id(case.kind, g);
                let gname = graph_name(case.kind, g);
                for t in 0..case.triples_per_graph {
                    let s = Term::Iri(format!("http://example.org/s/repeated/{g}/{}", t % 40));
                    let p = Term::Iri(format!("http://example.org/p/{}", t % 8));
                    let lex = format!("{}-{}", lex_pool[t % lex_pool.len()], t % 32);
                    let o = if t % 3 == 0 {
                        Term::Literal {
                            lex,
                            dt: Some(dt_pool[t % dt_pool.len()].into()),
                            lang: None,
                        }
                    } else {
                        Term::Literal {
                            lex,
                            dt: None,
                            lang: Some(lang_pool[t % lang_pool.len()].into()),
                        }
                    };
                    quints.push(Quint {
                        id: id.clone(),
                        s,
                        p,
                        o,
                        gname: gname.clone(),
                    });
                }
            }
            quints
        }
        WorkloadKind::HighCardinalityNames => {
            let mut quints = Vec::with_capacity(case.total_quads());
            for g in 0..case.n_graphs {
                let id = dataset_id(case.kind, g);
                let gname = graph_name(case.kind, g);
                for t in 0..case.triples_per_graph {
                    quints.push(Quint {
                        id: id.clone(),
                        s: Term::Iri(format!(
                            "https://entities.example.org/subject/{g:04}/{t:04}/{}",
                            50_000 + g * 10 + t
                        )),
                        p: Term::Iri(format!("https://schema.example.org/p/{}", t % 11)),
                        o: Term::Literal {
                            lex: format!("value-{g:04}-{t:04}"),
                            dt: Some("http://www.w3.org/2001/XMLSchema#string".into()),
                            lang: None,
                        },
                        gname: gname.clone(),
                    });
                }
            }
            quints
        }
    }
}

fn median_ms(times: &mut [f64]) -> f64 {
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if times.len() % 2 == 1 {
        times[times.len() / 2]
    } else {
        let hi = times.len() / 2;
        (times[hi - 1] + times[hi]) / 2.0
    }
}

fn measure_ms(iterations: usize, mut f: impl FnMut()) -> f64 {
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        f();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    median_ms(&mut samples)
}

fn read_all_graphs(file: &R5tuFile) {
    let graphs = file.enumerate_all().unwrap();
    for graph in graphs {
        for _ in file.triples_ids(graph.gid).unwrap() {}
    }
}

fn resolve_all_graphs(file: &R5tuFile, case: WorkloadCase) {
    for g in 0..case.n_graphs {
        let _ = file
            .resolve_gid(&dataset_id(case.kind, g), &graph_name(case.kind, g))
            .unwrap();
    }
}

fn section_len(file: &R5tuFile, kind: SectionKind) -> u64 {
    file.section(kind).map(|section| section.len).unwrap_or(0)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let out_dir = args.output_dir.unwrap_or_else(std::env::temp_dir);
    let opts = WriterOptions {
        zstd: args.zstd,
        with_crc: args.with_crc,
    };

    println!("[");
    for (index, case) in workload_cases(args.workload).into_iter().enumerate() {
        let quints = generate_quints(case);
        let path = out_dir.join(format!("rdf5d_workload_{}.r5tu", case.name));

        let write_ms = measure_ms(args.iterations, || {
            write_file_with_options(&path, &quints, opts).unwrap();
        });
        let file = R5tuFile::open(&path)?;
        let open_strict_ms = measure_ms(args.iterations, || {
            R5tuFile::open(&path).unwrap();
        });
        let open_trusted_ms = measure_ms(args.iterations, || {
            R5tuFile::open_with_options(
                &path,
                OpenOptions {
                    integrity: IntegrityMode::Trusted,
                    prefer_mmap: false,
                },
            )
            .unwrap();
        });
        #[cfg(feature = "mmap")]
        let open_mmap_ms = measure_ms(args.iterations, || {
            R5tuFile::open_with_options(
                &path,
                OpenOptions {
                    integrity: IntegrityMode::Structural,
                    prefer_mmap: true,
                },
            )
            .unwrap();
        });
        let read_all_ms = measure_ms(args.iterations, || read_all_graphs(&file));
        let resolve_all_ms = measure_ms(args.iterations, || resolve_all_graphs(&file, case));

        if index > 0 {
            println!(",");
        }
        println!("  {{");
        println!("    \"workload\": \"{}\",", case.name);
        println!("    \"graphs\": {},", case.n_graphs);
        println!("    \"triples_per_graph\": {},", case.triples_per_graph);
        println!("    \"total_quads\": {},", case.total_quads());
        println!("    \"file_bytes\": {},", std::fs::metadata(&path)?.len());
        println!("    \"write_ms\": {:.3},", write_ms);
        println!("    \"open_strict_ms\": {:.3},", open_strict_ms);
        println!("    \"open_trusted_ms\": {:.3},", open_trusted_ms);
        #[cfg(feature = "mmap")]
        println!("    \"open_mmap_ms\": {:.3},", open_mmap_ms);
        println!("    \"read_all_ms\": {:.3},", read_all_ms);
        println!("    \"resolve_all_ms\": {:.3},", resolve_all_ms);
        println!("    \"sections\": {{");
        println!(
            "      \"term_dict\": {},",
            section_len(&file, SectionKind::TermDict)
        );
        println!(
            "      \"id_dict\": {},",
            section_len(&file, SectionKind::IdDict)
        );
        println!(
            "      \"gname_dict\": {},",
            section_len(&file, SectionKind::GNameDict)
        );
        println!("      \"gdir\": {},", section_len(&file, SectionKind::GDir));
        println!(
            "      \"idx_id2gid\": {},",
            section_len(&file, SectionKind::IdxId2Gid)
        );
        println!(
            "      \"idx_gname2gid\": {},",
            section_len(&file, SectionKind::IdxGName2Gid)
        );
        println!(
            "      \"idx_pair2gid\": {},",
            section_len(&file, SectionKind::IdxPair2Gid)
        );
        println!(
            "      \"triple_blocks\": {}",
            section_len(&file, SectionKind::TripleBlocks)
        );
        println!("    }}");
        println!("  }}");
    }
    println!("]");
    Ok(())
}
