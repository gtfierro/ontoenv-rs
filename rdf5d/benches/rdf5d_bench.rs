use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rdf5d::{Quint, R5tuFile, StreamingWriter, Term, WriterOptions, write_file_with_options};
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

fn opts_plain() -> WriterOptions {
    WriterOptions {
        zstd: false,
        with_crc: true,
    }
}

fn opts_zstd() -> WriterOptions {
    WriterOptions {
        zstd: true,
        with_crc: true,
    }
}

fn bench_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("write");
    for n in [100, 1_000, 10_000] {
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

fn bench_write_zstd(c: &mut Criterion) {
    let mut group = c.benchmark_group("write_zstd");
    for n in [100, 1_000, 10_000] {
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

fn bench_write_streaming(c: &mut Criterion) {
    let mut group = c.benchmark_group("write_streaming");
    for n in [100, 1_000, 10_000] {
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
    for n in [100, 1_000, 10_000] {
        let quints = generate_quints(1, n);
        let f = NamedTempFile::new().unwrap();
        write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &f, |b, f| {
            b.iter(|| {
                R5tuFile::open(f.path()).unwrap();
            });
        });
    }
    group.finish();
}

fn bench_read_triples(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_triples");
    for n in [100, 1_000, 10_000] {
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

fn bench_graph_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_lookup");
    for n_graphs in [5, 20, 100] {
        let quints = generate_quints(n_graphs, 50);
        let f = NamedTempFile::new().unwrap();
        write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
        let file = R5tuFile::open(f.path()).unwrap();
        group.throughput(Throughput::Elements(n_graphs as u64));
        group.bench_with_input(
            BenchmarkId::new("enumerate_by_id", n_graphs),
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
            BenchmarkId::new("enumerate_by_graphname", n_graphs),
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
    group.finish();
}

fn bench_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("roundtrip");
    for n in [1_000, 10_000] {
        let quints = generate_quints(3, n);
        let total = quints.len() as u64;
        group.throughput(Throughput::Elements(total));
        group.bench_with_input(BenchmarkId::from_parameter(n), &quints, |b, quints| {
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
        });
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
    bench_graph_lookup,
    bench_roundtrip,
);
criterion_main!(benches);
