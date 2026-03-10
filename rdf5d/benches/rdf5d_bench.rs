use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rdf5d::{
    IntegrityMode, OpenOptions, Quint, R5tuFile, StreamingWriter, Term, WriterOptions,
    write_file_with_options,
};
use std::env;
use tempfile::NamedTempFile;

/// Generate a dataset of `n_triples` spread across `n_graphs` graphs.
/// Produces a mix of IRIs, bnodes, and literals (with and without lang/dt).
fn generate_quints(n_graphs: usize, triples_per_graph: usize) -> Vec<Quint> {
    let mut quints = Vec::with_capacity(n_graphs * triples_per_graph);
    for g in 0..n_graphs {
        let id = format!("dataset/{g}");
        let gname = format!("http://example.org/graph/{g}");
        for t in 0..triples_per_graph {
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

fn workload_cases() -> [WorkloadCase; 4] {
    [
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
    ]
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

fn generate_workload(case: WorkloadCase) -> Vec<Quint> {
    match case.kind {
        WorkloadKind::Balanced => generate_quints(case.n_graphs, case.triples_per_graph),
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

fn opts_plain() -> WriterOptions {
    WriterOptions {
        zstd: false,
        with_crc: true,
    }
}

#[cfg(feature = "zstd")]
fn opts_zstd() -> WriterOptions {
    WriterOptions {
        zstd: true,
        with_crc: true,
    }
}

fn bench_usize_list(var: &str, default: &[usize]) -> Vec<usize> {
    match env::var(var) {
        Ok(raw) => {
            let values: Vec<usize> = raw
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| {
                    value.parse::<usize>().unwrap_or_else(|_| {
                        panic!("invalid usize value '{value}' in {var}");
                    })
                })
                .collect();
            assert!(
                !values.is_empty(),
                "{var} must contain at least one positive integer"
            );
            assert!(
                values.iter().all(|&value| value > 0),
                "{var} must contain only positive integers"
            );
            values
        }
        Err(_) => default.to_vec(),
    }
}

fn bench_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("write");
    for n in bench_usize_list("RDF5D_BENCH_SINGLE_GRAPH_TRIPLES", &[100, 1_000, 10_000]) {
        let quints = generate_quints(1, n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &quints, |b, quints| {
            b.iter(|| {
                let f = NamedTempFile::new().unwrap();
                write_file_with_options(f.path(), quints, opts_plain()).unwrap();
            });
        });
    }
    group.finish();
}

#[cfg(feature = "zstd")]
fn bench_write_zstd(c: &mut Criterion) {
    let mut group = c.benchmark_group("write_zstd");
    for n in bench_usize_list("RDF5D_BENCH_SINGLE_GRAPH_TRIPLES", &[100, 1_000, 10_000]) {
        let quints = generate_quints(1, n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &quints, |b, quints| {
            b.iter(|| {
                let f = NamedTempFile::new().unwrap();
                write_file_with_options(f.path(), quints, opts_zstd()).unwrap();
            });
        });
    }
    group.finish();
}

#[cfg(not(feature = "zstd"))]
fn bench_write_zstd(_: &mut Criterion) {}

fn bench_write_streaming(c: &mut Criterion) {
    let mut group = c.benchmark_group("write_streaming");
    for n in bench_usize_list("RDF5D_BENCH_SINGLE_GRAPH_TRIPLES", &[100, 1_000, 10_000]) {
        let quints = generate_quints(1, n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &quints, |b, quints| {
            b.iter(|| {
                let f = NamedTempFile::new().unwrap();
                let mut w = StreamingWriter::new(f.path(), opts_plain());
                for q in quints {
                    w.add(q.clone()).unwrap();
                }
                w.finalize().unwrap();
            });
        });
    }
    group.finish();
}

fn bench_open(c: &mut Criterion) {
    let mut group = c.benchmark_group("open");
    for n in bench_usize_list("RDF5D_BENCH_SINGLE_GRAPH_TRIPLES", &[100, 1_000, 10_000]) {
        let quints = generate_quints(1, n);
        let f = NamedTempFile::new().unwrap();
        write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &f, |b, f| {
            b.iter(|| {
                R5tuFile::open(f.path()).unwrap();
            });
        });
        group.bench_with_input(BenchmarkId::new("structural", n), &f, |b, f| {
            b.iter(|| {
                R5tuFile::open_with_options(
                    f.path(),
                    OpenOptions {
                        integrity: IntegrityMode::Structural,
                        prefer_mmap: false,
                    },
                )
                .unwrap();
            });
        });
        group.bench_with_input(BenchmarkId::new("trusted", n), &f, |b, f| {
            b.iter(|| {
                R5tuFile::open_with_options(
                    f.path(),
                    OpenOptions {
                        integrity: IntegrityMode::Trusted,
                        prefer_mmap: false,
                    },
                )
                .unwrap();
            });
        });
        #[cfg(feature = "mmap")]
        group.bench_with_input(BenchmarkId::new("mmap_structural", n), &f, |b, f| {
            b.iter(|| {
                R5tuFile::open_with_options(
                    f.path(),
                    OpenOptions {
                        integrity: IntegrityMode::Structural,
                        prefer_mmap: true,
                    },
                )
                .unwrap();
            });
        });
    }
    group.finish();
}

fn bench_read_triples(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_triples");
    for n in bench_usize_list("RDF5D_BENCH_SINGLE_GRAPH_TRIPLES", &[100, 1_000, 10_000]) {
        let quints = generate_quints(1, n);
        let f = NamedTempFile::new().unwrap();
        write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
        let file = R5tuFile::open(f.path()).unwrap();
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &file, |b, file| {
            b.iter(|| {
                let iter = file.triples_ids(0).unwrap();
                for _ in iter {}
            });
        });
    }
    group.finish();
}

fn bench_first_triple(c: &mut Criterion) {
    let mut group = c.benchmark_group("first_triple");
    for n in bench_usize_list("RDF5D_BENCH_SINGLE_GRAPH_TRIPLES", &[100, 1_000, 10_000]) {
        let quints = generate_quints(1, n);
        let f = NamedTempFile::new().unwrap();
        write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
        let file = R5tuFile::open(f.path()).unwrap();
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(n), &file, |b, file| {
            b.iter(|| {
                let mut iter = file.triples_ids(0).unwrap();
                iter.next().unwrap()
            });
        });
    }
    group.finish();
}

fn bench_graph_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_lookup");
    let graph_counts = bench_usize_list("RDF5D_BENCH_GRAPH_COUNTS", &[5, 20, 100]);
    let triples_per_graph = bench_usize_list("RDF5D_BENCH_GRAPH_TRIPLES_PER_GRAPH", &[50]);
    for n_graphs in graph_counts {
        for triples in &triples_per_graph {
            let quints = generate_quints(n_graphs, *triples);
            let f = NamedTempFile::new().unwrap();
            write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
            let file = R5tuFile::open(f.path()).unwrap();
            group.throughput(Throughput::Elements((n_graphs * *triples) as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("enumerate_by_id/tpg={triples}"), n_graphs),
                &file,
                |b, file| {
                    b.iter(|| {
                        for g in 0..n_graphs {
                            let _ = file.enumerate_by_id(&format!("dataset/{g}")).unwrap();
                        }
                    });
                },
            );
            group.bench_with_input(
                BenchmarkId::new(format!("enumerate_by_graphname/tpg={triples}"), n_graphs),
                &file,
                |b, file| {
                    b.iter(|| {
                        for g in 0..n_graphs {
                            let _ = file
                                .enumerate_by_graphname(&format!("http://example.org/graph/{g}"))
                                .unwrap();
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_resolve_gid(c: &mut Criterion) {
    let mut group = c.benchmark_group("resolve_gid");
    let graph_counts = bench_usize_list("RDF5D_BENCH_GRAPH_COUNTS", &[5, 20, 100]);
    let triples_per_graph = bench_usize_list("RDF5D_BENCH_GRAPH_TRIPLES_PER_GRAPH", &[50]);
    for n_graphs in graph_counts {
        for triples in &triples_per_graph {
            let quints = generate_quints(n_graphs, *triples);
            let f = NamedTempFile::new().unwrap();
            write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
            let file = R5tuFile::open(f.path()).unwrap();
            group.throughput(Throughput::Elements((n_graphs * *triples) as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("tpg={triples}"), n_graphs),
                &file,
                |b, file| {
                    b.iter(|| {
                        for g in 0..n_graphs {
                            let _ = file
                                .resolve_gid(
                                    &format!("dataset/{g}"),
                                    &format!("http://example.org/graph/{g}"),
                                )
                                .unwrap();
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_enumerate_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("enumerate_all");
    let graph_counts =
        bench_usize_list("RDF5D_BENCH_ENUMERATE_ALL_GRAPH_COUNTS", &[5, 20, 100, 500]);
    let triples_per_graph = bench_usize_list("RDF5D_BENCH_ENUMERATE_ALL_TRIPLES_PER_GRAPH", &[10]);
    for n_graphs in graph_counts {
        for triples in &triples_per_graph {
            let quints = generate_quints(n_graphs, *triples);
            let f = NamedTempFile::new().unwrap();
            write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
            let file = R5tuFile::open(f.path()).unwrap();
            group.throughput(Throughput::Elements((n_graphs * *triples) as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("tpg={triples}"), n_graphs),
                &file,
                |b, file| {
                    b.iter(|| {
                        let _ = file.enumerate_all().unwrap();
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("roundtrip");
    let graph_counts = bench_usize_list("RDF5D_BENCH_ROUNDTRIP_GRAPH_COUNTS", &[3]);
    let triples_per_graph =
        bench_usize_list("RDF5D_BENCH_ROUNDTRIP_TRIPLES_PER_GRAPH", &[1_000, 10_000]);
    for n_graphs in graph_counts {
        for triples in &triples_per_graph {
            let quints = generate_quints(n_graphs, *triples);
            let total = quints.len() as u64;
            group.throughput(Throughput::Elements(total));
            group.bench_with_input(
                BenchmarkId::new(format!("graphs={n_graphs}"), triples),
                &quints,
                |b, quints| {
                    b.iter(|| {
                        let f = NamedTempFile::new().unwrap();
                        write_file_with_options(f.path(), quints, opts_plain()).unwrap();
                        let file = R5tuFile::open(f.path()).unwrap();
                        let graphs = file.enumerate_all().unwrap();
                        for gr in &graphs {
                            let iter = file.triples_ids(gr.gid).unwrap();
                            for _ in iter {}
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_workload_matrix(c: &mut Criterion) {
    let mut group = c.benchmark_group("workload_matrix");
    for case in workload_cases() {
        let quints = generate_workload(case);
        let total = case.total_quads() as u64;
        let f = NamedTempFile::new().unwrap();
        write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
        let file = R5tuFile::open(f.path()).unwrap();

        group.throughput(Throughput::Elements(total));
        group.bench_with_input(
            BenchmarkId::new("write", case.name),
            &quints,
            |b, quints| {
                b.iter(|| {
                    let f = NamedTempFile::new().unwrap();
                    write_file_with_options(f.path(), quints, opts_plain()).unwrap();
                });
            },
        );
        group.bench_with_input(BenchmarkId::new("open_strict", case.name), &f, |b, f| {
            b.iter(|| {
                R5tuFile::open(f.path()).unwrap();
            });
        });
        group.bench_with_input(BenchmarkId::new("open_trusted", case.name), &f, |b, f| {
            b.iter(|| {
                R5tuFile::open_with_options(
                    f.path(),
                    OpenOptions {
                        integrity: IntegrityMode::Trusted,
                        prefer_mmap: false,
                    },
                )
                .unwrap();
            });
        });
        #[cfg(feature = "mmap")]
        group.bench_with_input(BenchmarkId::new("open_mmap", case.name), &f, |b, f| {
            b.iter(|| {
                R5tuFile::open_with_options(
                    f.path(),
                    OpenOptions {
                        integrity: IntegrityMode::Structural,
                        prefer_mmap: true,
                    },
                )
                .unwrap();
            });
        });
        group.bench_with_input(BenchmarkId::new("read_all", case.name), &file, |b, file| {
            b.iter(|| read_all_graphs(file));
        });
        group.bench_with_input(
            BenchmarkId::new("resolve_all", case.name),
            &file,
            |b, file| {
                b.iter(|| resolve_all_graphs(file, case));
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_write,
    bench_write_zstd,
    bench_write_streaming,
    bench_open,
    bench_read_triples,
    bench_first_triple,
    bench_graph_lookup,
    bench_resolve_gid,
    bench_enumerate_all,
    bench_roundtrip,
    bench_workload_matrix,
);
criterion_main!(benches);
