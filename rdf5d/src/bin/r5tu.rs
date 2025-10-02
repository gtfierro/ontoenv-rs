use clap::{Args, Parser, Subcommand, ValueEnum};
#[cfg(feature = "oxigraph")]
use oxigraph::io::RdfFormat;
#[cfg(feature = "oxigraph")]
use oxigraph::model::GraphNameRef;
#[cfg(feature = "oxigraph")]
use oxigraph::store::Store;
#[cfg(feature = "oxigraph")]
use std::fs::File;
#[cfg(feature = "oxigraph")]
use std::io::BufReader;
use std::path::PathBuf;
#[cfg(feature = "oxigraph")]
use std::time::Instant;

#[cfg(feature = "oxigraph")]
use rdf5d::writer::WriterOptions;
#[cfg(feature = "oxigraph")]
use rdf5d::{Quint, R5tuFile, StreamingWriter, Term};

#[derive(Clone, Copy, ValueEnum)]
enum GraphFmt {
    Turtle,
    Ntriples,
    Rdfxml,
}
#[derive(Clone, Copy, ValueEnum)]
enum DatasetFmt {
    Trig,
    Nquads,
}

#[derive(Parser)]
#[command(name = "r5tu", version, about = "R5TU builder/stat CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    BuildGraph(BuildGraphArgs),
    BuildDataset(BuildDatasetArgs),
    Stat(StatArgs),
}

#[derive(Args)]
struct BuildGraphArgs {
    #[arg(long = "input", required = true)]
    input: Vec<PathBuf>,
    #[arg(long = "output")]
    output: PathBuf,
    #[arg(long = "format", value_enum)]
    format: Option<GraphFmt>,
    #[arg(long = "id")]
    id: Option<String>,
    #[arg(long = "graphname")]
    graphname: Option<String>,
    #[arg(long = "zstd", default_value_t = false)]
    zstd: bool,
    #[arg(long = "no-crc", default_value_t = false)]
    no_crc: bool,
}

#[derive(Args)]
struct BuildDatasetArgs {
    #[arg(long = "input", required = true)]
    input: Vec<PathBuf>,
    #[arg(long = "output")]
    output: PathBuf,
    #[arg(long = "format", value_enum)]
    format: Option<DatasetFmt>,
    #[arg(long = "id")]
    id: Option<String>,
    #[arg(long = "default-graphname")]
    default_graphname: Option<String>,
    #[arg(long = "zstd", default_value_t = false)]
    zstd: bool,
    #[arg(long = "no-crc", default_value_t = false)]
    no_crc: bool,
}

#[derive(Args)]
struct StatArgs {
    #[arg(long = "file")]
    file: PathBuf,
    #[arg(long = "verbose", default_value_t = false)]
    verbose: bool,
    #[arg(long = "graphname")]
    graphname: Option<String>,
    #[arg(long = "list", default_value_t = false)]
    list: bool,
    #[cfg(feature = "mmap")]
    #[arg(
        long = "no-mmap",
        default_value_t = false,
        help = "Disable mmap and read into memory"
    )]
    no_mmap: bool,
}

#[cfg(feature = "oxigraph")]
fn infer_graph_rdf_format(ext: &str) -> Option<RdfFormat> {
    match ext.to_ascii_lowercase().as_str() {
        "nt" | "ntriples" => Some(RdfFormat::NTriples),
        "ttl" | "turtle" => Some(RdfFormat::Turtle),
        "rdf" | "xml" | "rdfxml" => Some(RdfFormat::RdfXml),
        _ => None,
    }
}
#[cfg(feature = "oxigraph")]
fn infer_dataset_rdf_format(ext: &str) -> Option<RdfFormat> {
    match ext.to_ascii_lowercase().as_str() {
        "nq" | "nquads" => Some(RdfFormat::NQuads),
        "trig" => Some(RdfFormat::TriG),
        _ => None,
    }
}

#[cfg(feature = "oxigraph")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Commands::BuildGraph(args) => {
            let opts = WriterOptions {
                zstd: args.zstd,
                with_crc: !args.no_crc,
            };
            let mut w = StreamingWriter::new(&args.output, opts);
            let start = Instant::now();
            for input in args.input {
                let f = File::open(&input)?;
                let mut rdr = BufReader::new(f);
                let rfmt: RdfFormat = match args.format {
                    Some(GraphFmt::Turtle) => RdfFormat::Turtle,
                    Some(GraphFmt::Ntriples) => RdfFormat::NTriples,
                    Some(GraphFmt::Rdfxml) => RdfFormat::RdfXml,
                    None => infer_graph_rdf_format(
                        input.extension().and_then(|e| e.to_str()).unwrap_or(""),
                    )
                    .unwrap_or(RdfFormat::Turtle),
                };
                // Load into store via BulkLoader (explicit fast path)
                let store = Store::new()?;
                let mut loader = store.bulk_loader();
                loader.load_from_reader(rfmt, &mut rdr)?;
                loader.commit()?;
                let gname_auto =
                    rdf5d::writer::detect_graphname_from_store(&store).unwrap_or_else(|| {
                        args.graphname
                            .clone()
                            .unwrap_or_else(|| "default".to_string())
                    });
                let id = args
                    .id
                    .clone()
                    .unwrap_or_else(|| input.to_string_lossy().to_string());
                // Stream loaded triples from default graph into our writer
                let mut n = 0usize;
                for q in store.quads_for_pattern(None, None, None, Some(GraphNameRef::DefaultGraph))
                {
                    let q = q?;
                    n += 1;
                    let s = match q.subject {
                        oxigraph::model::NamedOrBlankNode::NamedNode(nm) => {
                            Term::Iri(nm.as_str().to_string())
                        }
                        oxigraph::model::NamedOrBlankNode::BlankNode(b) => {
                            Term::BNode(format!("_:{}", b.as_str()))
                        }
                    };
                    let p = Term::Iri(q.predicate.as_str().to_string());
                    let o = match q.object {
                        oxigraph::model::Term::NamedNode(nm) => Term::Iri(nm.as_str().to_string()),
                        oxigraph::model::Term::BlankNode(b) => {
                            Term::BNode(format!("_:{}", b.as_str()))
                        }
                        oxigraph::model::Term::Literal(l) => {
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
                    };
                    w.add(Quint {
                        id: id.clone(),
                        s,
                        p,
                        o,
                        gname: gname_auto.clone(),
                    })?;
                }
                println!(
                    "Added graph id='{}' graphname='{}' ({} triples) from '{}'",
                    id,
                    gname_auto,
                    n,
                    input.display()
                );
            }
            w.finalize()?;
            eprintln!("built in {:?}", start.elapsed());
        }
        Commands::BuildDataset(args) => {
            let default_g = args
                .default_graphname
                .clone()
                .unwrap_or_else(|| "default".to_string());
            let opts = WriterOptions {
                zstd: args.zstd,
                with_crc: !args.no_crc,
            };
            let mut w = StreamingWriter::new(&args.output, opts);
            let start = Instant::now();
            for input in args.input {
                let f = File::open(&input)?;
                let mut rdr = BufReader::new(f);
                let rfmt: RdfFormat = match args.format {
                    Some(DatasetFmt::Trig) => RdfFormat::TriG,
                    Some(DatasetFmt::Nquads) => RdfFormat::NQuads,
                    None => infer_dataset_rdf_format(
                        input.extension().and_then(|e| e.to_str()).unwrap_or(""),
                    )
                    .unwrap_or(RdfFormat::NQuads),
                };
                let store = Store::new()?;
                store.load_from_reader(rfmt, &mut rdr)?;
                let id = args
                    .id
                    .clone()
                    .unwrap_or_else(|| input.to_string_lossy().to_string());
                let mut n = 0usize;
                for q in store.quads_for_pattern(None, None, None, None) {
                    let q = q?;
                    n += 1;
                    let s = match q.subject {
                        oxigraph::model::NamedOrBlankNode::NamedNode(nm) => {
                            Term::Iri(nm.as_str().to_string())
                        }
                        oxigraph::model::NamedOrBlankNode::BlankNode(b) => {
                            Term::BNode(format!("_:{}", b.as_str()))
                        }
                    };
                    let p = Term::Iri(q.predicate.as_str().to_string());
                    let o = match q.object {
                        oxigraph::model::Term::NamedNode(nm) => Term::Iri(nm.as_str().to_string()),
                        oxigraph::model::Term::BlankNode(b) => {
                            Term::BNode(format!("_:{}", b.as_str()))
                        }
                        oxigraph::model::Term::Literal(l) => {
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
                    };
                    let gname = match q.graph_name {
                        oxigraph::model::GraphName::DefaultGraph => default_g.clone(),
                        oxigraph::model::GraphName::NamedNode(nm) => nm.as_str().to_string(),
                        oxigraph::model::GraphName::BlankNode(b) => format!("_:{}", b.as_str()),
                    };
                    w.add(Quint {
                        id: id.clone(),
                        s,
                        p,
                        o,
                        gname,
                    })?;
                }
                println!(
                    "Added dataset id='{}' quads={} from '{}'",
                    id,
                    n,
                    input.display()
                );
            }
            w.finalize()?;
            eprintln!("built in {:?}", start.elapsed());
        }
        Commands::Stat(args) => {
            let file = args.file;
            let f = match { R5tuFile::open(&file) } {
                Ok(f) => f,
                Err(e) => {
                    eprintln!(
                        "stat: failed to open '{}': {}\nHint: Use 'build-graph' or 'build-dataset' to produce an .r5tu file first.",
                        file.display(),
                        e
                    );
                    std::process::exit(2);
                }
            };
            let verbose = args.verbose;
            let list = args.list;
            let filter_g = args.graphname;
            let toc = f.toc();
            eprintln!("sections: {}", toc.len());
            if verbose {
                let h = f.header();
                eprintln!(
                    "header.magic='{}' version={} flags=0x{:04x} created_unix={} toc_off={} toc_len={}",
                    std::str::from_utf8(&h.magic).unwrap_or("????"),
                    h.version_u16,
                    h.flags_u16,
                    h.created_unix64,
                    h.toc_off_u64,
                    h.toc_len_u32
                );
                for (i, e) in toc.iter().enumerate() {
                    eprintln!(
                        "  [{}] kind={:?} off={} len={} crc={}",
                        i, e.kind, e.section.off, e.section.len, e.crc32_u32
                    );
                }
            }
            let start = Instant::now();
            let mut n_triples = 0u64;
            let graphs = if let Some(ref gname) = filter_g {
                f.enumerate_by_graphname(gname)?
            } else {
                f.enumerate_all()?
            };
            let n_graphs = graphs.len() as u64;
            for gr in &graphs {
                n_triples += gr.n_triples;
            }
            eprintln!(
                "graphs: {} triples: {} in {:?}",
                n_graphs,
                n_triples,
                start.elapsed()
            );
            if list {
                for gr in graphs {
                    println!(
                        "gid={} id='{}' graphname='{}' n_triples={}",
                        gr.gid, gr.id, gr.graphname, gr.n_triples
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(not(feature = "oxigraph"))]
fn main() {
    eprintln!(
        "r5tu CLI requires the 'oxigraph' feature. Try: cargo run --features oxigraph --bin r5tu -- help"
    );
}
