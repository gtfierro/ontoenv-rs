use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use rdf5d::{
    IntegrityMode, OpenOptions, Pattern, Quint, R5tuFile, Snapshot, StreamingWriter, Term, View,
    WriterOptions, write_file_with_options,
};
use std::env;
#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
use std::path::Path;
use tempfile::NamedTempFile;

#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
use oxigraph::io::{RdfFormat, RdfParser};
#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
use oxigraph::model::{
    GraphName, Literal as OxLiteral, NamedNode as OxNamedNode, NamedOrBlankNode, Quad,
    Term as OxTerm,
};
#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
use oxigraph::sparql::{QueryResults as OxQueryResults, SparqlEvaluator};
#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
use oxigraph::store::Store;
#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
use spareval::QueryResults as R5QueryResults;
#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
use spargebra::SparqlParser;
#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
use tempfile::TempDir;

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

#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
fn r5_term_to_ox(term: &Term) -> OxTerm {
    match term {
        Term::Iri(value) => OxNamedNode::new(value.clone()).unwrap().into(),
        Term::BNode(value) => {
            let label = value.strip_prefix("_:").unwrap_or(value);
            oxigraph::model::BlankNode::new(label.to_string())
                .unwrap()
                .into()
        }
        Term::Literal { lex, dt, lang } => {
            if let Some(dt) = dt {
                OxLiteral::new_typed_literal(lex.clone(), OxNamedNode::new(dt.clone()).unwrap())
                    .into()
            } else if let Some(lang) = lang {
                OxLiteral::new_language_tagged_literal(lex.clone(), lang.clone())
                    .unwrap()
                    .into()
            } else {
                OxLiteral::new_simple_literal(lex.clone()).into()
            }
        }
    }
}

#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
fn quint_to_quad(quint: &Quint) -> Quad {
    let subject: NamedOrBlankNode = match &quint.s {
        Term::Iri(value) => OxNamedNode::new(value.clone()).unwrap().into(),
        Term::BNode(value) => {
            let label = value.strip_prefix("_:").unwrap_or(value);
            oxigraph::model::BlankNode::new(label.to_string())
                .unwrap()
                .into()
        }
        Term::Literal { .. } => panic!("literal subject is invalid"),
    };
    let predicate = match &quint.p {
        Term::Iri(value) => OxNamedNode::new(value.clone()).unwrap(),
        _ => panic!("non-IRI predicate is invalid"),
    };
    let object = r5_term_to_ox(&quint.o);
    let graph = GraphName::NamedNode(OxNamedNode::new(quint.gname.clone()).unwrap());
    Quad::new(subject, predicate, object, graph)
}

#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
fn build_rocksdb_store(quints: &[Quint]) -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut loader = store.bulk_loader();
    loader.load_quads(quints.iter().map(quint_to_quad)).unwrap();
    loader.commit().unwrap();
    (dir, store)
}

#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
fn ox_quad_to_quint(quad: Quad, id: &str, graph_name: &str) -> Quint {
    fn ox_term_to_r5(term: OxTerm) -> Term {
        match term {
            OxTerm::NamedNode(node) => Term::Iri(node.into_string()),
            OxTerm::BlankNode(node) => Term::BNode(node.as_str().to_string()),
            OxTerm::Literal(literal) => {
                if let Some(language) = literal.language() {
                    Term::Literal {
                        lex: literal.value().to_string(),
                        dt: None,
                        lang: Some(language.to_string()),
                    }
                } else {
                    Term::Literal {
                        lex: literal.value().to_string(),
                        dt: Some(literal.datatype().as_str().to_string()),
                        lang: None,
                    }
                }
            }
        }
    }

    Quint {
        id: id.to_string(),
        s: match quad.subject {
            NamedOrBlankNode::NamedNode(node) => Term::Iri(node.into_string()),
            NamedOrBlankNode::BlankNode(node) => Term::BNode(node.as_str().to_string()),
        },
        p: Term::Iri(quad.predicate.into_string()),
        o: ox_term_to_r5(quad.object),
        gname: graph_name.to_string(),
    }
}

#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
fn load_brick_quints(path: &Path, id: &str, graph_name: &str) -> Vec<Quint> {
    let store = Store::new().unwrap();
    let parser = RdfParser::from_format(RdfFormat::Turtle)
        .with_default_graph(GraphName::NamedNode(OxNamedNode::new(graph_name).unwrap()));
    let mut loader = store.bulk_loader();
    loader
        .load_from_reader(parser, std::fs::File::open(path).unwrap())
        .unwrap();
    loader.commit().unwrap();
    store
        .iter()
        .map(|quad| ox_quad_to_quint(quad.unwrap(), id, graph_name))
        .collect()
}

#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
fn count_rdf5d_solutions(snapshot: &Snapshot, query: &spargebra::Query) -> usize {
    let mut query = query.clone();
    match snapshot.query(&mut query).unwrap() {
        R5QueryResults::Solutions(solutions) => solutions.count(),
        R5QueryResults::Boolean(value) => usize::from(value),
        R5QueryResults::Graph(triples) => triples.count(),
    }
}

#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
fn count_oxigraph_solutions(store: &Store, query: &spargebra::Query) -> usize {
    match SparqlEvaluator::new()
        .for_query(query.clone())
        .on_store(store)
        .execute()
        .unwrap()
    {
        OxQueryResults::Solutions(solutions) => solutions.count(),
        OxQueryResults::Boolean(value) => usize::from(value),
        OxQueryResults::Graph(triples) => triples.count(),
    }
}

#[cfg(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql"))]
fn bench_sparql_backends(c: &mut Criterion) {
    let mut group = c.benchmark_group("sparql_backends");
    let brick_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("brick")
        .join("Brick.ttl");
    let graph_name = "urn:ontoenv:bench:brick";
    let quints = load_brick_quints(&brick_path, "brick:Brick.ttl", graph_name);
    let total = quints.len() as u64;
    let file_handle = NamedTempFile::new().unwrap();
    write_file_with_options(file_handle.path(), &quints, opts_plain()).unwrap();
    let snapshot = Snapshot::open(file_handle.path()).unwrap();
    let (_rocks_dir, store) = build_rocksdb_store(&quints);

    let graph_query = SparqlParser::new()
        .parse_query(&format!(
            "SELECT ?s ?o WHERE {{
               GRAPH <{graph_name}> {{
                 ?s <http://www.w3.org/2000/01/rdf-schema#label> ?o
               }}
             }}"
        ))
        .unwrap();
    let scan_query = SparqlParser::new()
        .parse_query(
            "SELECT ?g ?s ?o WHERE {
               GRAPH ?g {
                 ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> ?o
               }
             }",
        )
        .unwrap();

    group.throughput(Throughput::Elements(total));
    group.bench_with_input(
        BenchmarkId::from_parameter("rdf5d_graph_brick"),
        &snapshot,
        |b, snapshot| {
            b.iter(|| black_box(count_rdf5d_solutions(snapshot, &graph_query)));
        },
    );
    group.bench_with_input(
        BenchmarkId::from_parameter("rocksdb_graph_brick"),
        &store,
        |b, store| {
            b.iter(|| black_box(count_oxigraph_solutions(store, &graph_query)));
        },
    );
    group.bench_with_input(
        BenchmarkId::from_parameter("rdf5d_scan_brick"),
        &snapshot,
        |b, snapshot| {
            b.iter(|| black_box(count_rdf5d_solutions(snapshot, &scan_query)));
        },
    );
    group.bench_with_input(
        BenchmarkId::from_parameter("rocksdb_scan_brick"),
        &store,
        |b, store| {
            b.iter(|| black_box(count_oxigraph_solutions(store, &scan_query)));
        },
    );
    group.finish();
}

#[cfg(not(all(feature = "oxigraph", feature = "rocksdb", feature = "sparql")))]
fn bench_sparql_backends(_: &mut Criterion) {}

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

fn bench_view_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("view_creation");
    for case in workload_cases() {
        let quints = generate_workload(case);
        let f = NamedTempFile::new().unwrap();
        write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
        let snap = Snapshot::open(f.path()).unwrap();
        let total = case.total_quads() as u64;
        group.throughput(Throughput::Elements(total));

        // Collect graph names
        let names: Vec<&str> = snap.graph_names().collect();

        group.bench_with_input(
            BenchmarkId::new("from_names", case.name),
            &names,
            |b, names| {
                b.iter(|| {
                    let _view = View::from_names(&snap, names);
                });
            },
        );
    }
    group.finish();
}

fn bench_view_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("view_scan");
    for case in workload_cases() {
        let quints = generate_workload(case);
        let f = NamedTempFile::new().unwrap();
        write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
        let snap = Snapshot::open(f.path()).unwrap();
        let names: Vec<&str> = snap.graph_names().collect();
        let view = View::from_names(&snap, &names);
        let total = case.total_quads() as u64;
        group.throughput(Throughput::Elements(total));

        // All-unbound scan (full scan)
        group.bench_with_input(BenchmarkId::new("scan_all", case.name), &view, |b, view| {
            b.iter(|| {
                let count: usize = view.scan(Pattern::ANY).filter(|r| r.is_ok()).count();
                black_box(count);
            });
        });

        // Bound-predicate scan (uses PSO index, built on first call)
        group.bench_with_input(BenchmarkId::new("scan_bp", case.name), &view, |b, view| {
            // Use predicate "p/0" which exists in every workload
            let p_id = snap
                .file()
                .term_id(&rdf5d::DecodedTerm::Iri(std::borrow::Cow::Borrowed(
                    "http://example.org/p/0",
                )))
                .unwrap_or(0);
            let pat = Pattern {
                s: None,
                p: Some(p_id),
                o: None,
            };
            b.iter(|| {
                let count: usize = view.scan(pat).filter(|r| r.is_ok()).count();
                black_box(count);
            });
        });

        // First scan (triggers index build) vs cached for all-unbound
        group.bench_with_input(
            BenchmarkId::new("scan_all_first", case.name),
            &snap,
            |b, snap| {
                b.iter_with_setup(
                    || {
                        // Fresh view each iteration so indexes aren't cached
                        View::from_names(snap, &names)
                    },
                    |view| {
                        let count: usize = view.scan(Pattern::ANY).filter(|r| r.is_ok()).count();
                        black_box(count);
                    },
                );
            },
        );
    }
    group.finish();
}

fn bench_view_subset_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("view_subset_scan");
    for case in workload_cases() {
        let quints = generate_workload(case);
        let f = NamedTempFile::new().unwrap();
        write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
        let snap = Snapshot::open(f.path()).unwrap();
        let names: Vec<&str> = snap.graph_names().collect();
        let total = case.total_quads() as u64;
        group.throughput(Throughput::Elements(total));

        if names.len() > 1 {
            // View over half the graphs
            let half = names.len() / 2;
            let view = View::from_names(&snap, &names[..half]);
            group.bench_with_input(
                BenchmarkId::new("half_graphs", case.name),
                &view,
                |b, view| {
                    b.iter(|| {
                        let count: usize = view.scan(Pattern::ANY).filter(|r| r.is_ok()).count();
                        black_box(count);
                    });
                },
            );

            // View over a single graph
            let single = View::from_names(&snap, &names[..1]);
            group.bench_with_input(
                BenchmarkId::new("single_graph", case.name),
                &single,
                |b, view| {
                    b.iter(|| {
                        let count: usize = view.scan(Pattern::ANY).filter(|r| r.is_ok()).count();
                        black_box(count);
                    });
                },
            );
        }
    }
    group.finish();
}

#[cfg(feature = "sparql")]
fn bench_view_sparql(c: &mut Criterion) {
    use spargebra::SparqlParser;

    let mut group = c.benchmark_group("view_sparql");
    for case in workload_cases() {
        let quints = generate_workload(case);
        let f = NamedTempFile::new().unwrap();
        write_file_with_options(f.path(), &quints, opts_plain()).unwrap();
        let snap = Snapshot::open(f.path()).unwrap();
        let names: Vec<&str> = snap.graph_names().collect();
        let view = View::from_names(&snap, &names);
        let total = case.total_quads() as u64;
        group.throughput(Throughput::Elements(total));

        // SPARQL query against the view
        let query = SparqlParser::new()
            .parse_query(
                "SELECT (COUNT(?s) AS ?c) WHERE { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> ?o }",
            )
            .expect("bench query");

        group.bench_with_input(
            BenchmarkId::new("type_count", case.name),
            &(&snap, &query, &view),
            |b, &(_snap, query, view)| {
                b.iter(|| {
                    // First call against view builds its indexes
                    let mut q = query.clone();
                    let results = view.query(&mut q).unwrap();
                    black_box(results);
                });
            },
        );

        // Compare: Snapshot query (store-wide indexes)
        group.bench_with_input(
            BenchmarkId::new("snapshot_type_count", case.name),
            &(&snap, &query),
            |b, &(snap, query)| {
                b.iter(|| {
                    let mut q = query.clone();
                    let results = snap.query(&mut q).unwrap();
                    black_box(results);
                });
            },
        );
    }
    group.finish();
}

#[cfg(not(feature = "sparql"))]
fn bench_view_sparql(_: &mut Criterion) {}

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
    bench_view_creation,
    bench_view_scan,
    bench_view_subset_scan,
    bench_view_sparql,
    bench_sparql_backends,
);
criterion_main!(benches);
