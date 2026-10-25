//! Diagnostic worker for lib/tests/support/snapshot_workload.py.
//! prepare ROOT GRAPHS TRIPLES | read ROOT [hold|hold-release] | update ROOT TRIPLES BURST

use anyhow::{Context, Result};
use ontoenv::config::Config;
use ontoenv::ontology::OntologyLocation;
use ontoenv::snapshots::{EnvironmentSnapshot, SnapshotReader, SnapshotWriter};
use oxigraph::io::RdfFormat;
use rdf5d::{DecodedTerm, Pattern, Scope};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::io::{BufRead, Write as _};
use std::path::Path;
use std::sync::{mpsc, Arc};
use std::time::Instant;

fn source(graph: usize, triples: usize, revision: usize) -> Vec<u8> {
    let mut turtle = format!("<urn:graph:{graph}> a <http://www.w3.org/2002/07/owl#Ontology> .\n");
    for triple in 0..triples {
        let mut literal = String::new();
        for part in 0..4 {
            write!(
                literal,
                "{}",
                blake3::hash(format!("{graph}:{triple}:{part}:{revision}").as_bytes()).to_hex()
            )
            .unwrap();
        }
        let predicate = if triple == 42 {
            "urn:rare"
        } else {
            "urn:value"
        };
        writeln!(
            turtle,
            "<urn:subject:{graph}:{triple}> <{predicate}> \"{literal}\" ."
        )
        .unwrap();
    }
    turtle.into_bytes()
}

fn location(graph: usize) -> OntologyLocation {
    OntologyLocation::InMemory {
        identifier: format!("graph-{graph}"),
    }
}

fn emit(value: Value) -> Result<()> {
    println!("{value}");
    std::io::stdout().flush()?;
    Ok(())
}

fn memory() -> Value {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| -> Option<u64> {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    };
    json!({"rss_kib": field("VmRSS:"), "peak_rss_kib": field("VmHWM:")})
}

fn milliseconds(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn query(snapshot: &EnvironmentSnapshot) -> Result<usize> {
    let rdf = snapshot.rdf();
    let predicate = rdf
        .file()
        .term_id(&DecodedTerm::Iri("urn:rare".into()))
        .context("Missing rare predicate")?;
    let count = rdf
        .grouped_scan(
            Pattern {
                p: Some(predicate),
                ..Pattern::ANY
            },
            Scope::All,
        )?
        .count();
    anyhow::ensure!(
        count == snapshot.ontologies().len(),
        "Unexpected selective query result count"
    );
    Ok(count)
}

fn percentile(samples: &mut [f64], percent: usize) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[(samples.len() - 1) * percent / 100]
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mode = args.first().context("Missing mode")?.as_str();
    let root = Path::new(args.get(1).context("Missing root")?);
    match mode {
        "prepare" => {
            let graphs: usize = args.get(2).context("Missing graph count")?.parse()?;
            let triples: usize = args.get(3).context("Missing triple count")?.parse()?;
            anyhow::ensure!(graphs > 0 && triples > 42);
            let config = Config::builder()
                .root(root.to_path_buf())
                .temporary(false)
                .strict(true)
                .offline(true)
                .locations(vec![])
                .build()?;
            let mut writer = SnapshotWriter::create(config)?;
            let start = Instant::now();
            let mut update = writer.begin_update();
            for graph in 0..graphs {
                update.add_from_bytes(
                    location(graph),
                    source(graph, triples, 0),
                    Some(RdfFormat::Turtle),
                )?;
            }
            let staging = milliseconds(start);
            let start = Instant::now();
            update.commit()?;
            emit(
                json!({"kind":"prepared", "graphs":graphs, "triples":graphs * (triples + 1),
                "staging_ms":staging, "commit_ms":milliseconds(start), "memory":memory()}),
            )?;
        }
        "read" => {
            let start = Instant::now();
            let mut reader = SnapshotReader::open(root)?;
            let open = milliseconds(start);
            let retained = reader.snapshot();
            let id = retained
                .ontologies()
                .keys()
                .find(|id| id.name().as_str() == "urn:graph:0")
                .context("Missing graph zero")?
                .clone();
            let start = Instant::now();
            let graph = retained.copy_graph(&id)?;
            let graph_read = milliseconds(start);
            let start = Instant::now();
            query(&retained)?;
            let first_query = milliseconds(start);
            let mut warm = Vec::new();
            for _ in 0..30 {
                let start = Instant::now();
                query(&retained)?;
                warm.push(milliseconds(start));
            }
            emit(
                json!({"kind":"reader_ready", "pid":std::process::id(), "generation":retained.info().generation,
                "open_ms":open, "first_graph_ms":graph_read, "first_index_query_ms":first_query,
                "warm_query_p50_ms":percentile(&mut warm, 50), "memory":memory()}),
            )?;
            if args
                .get(2)
                .is_some_and(|value| value == "hold" || value == "hold-release")
            {
                let retain_old = args[2] == "hold";
                let retained_generation = retained.info().generation;
                let old_rdf = Arc::downgrade(&retained.rdf());
                let (send, receive) = mpsc::channel();
                std::thread::spawn(move || {
                    for line in std::io::stdin().lock().lines() {
                        if send.send(line).is_err() {
                            break;
                        }
                    }
                });
                let mut samples = Vec::new();
                let mut count = 0u64;
                let mut maximum = 0.0_f64;
                loop {
                    match receive.try_recv() {
                        Ok(command) => {
                            anyhow::ensure!(command? == "refresh");
                            break;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            anyhow::bail!("Controller disconnected")
                        }
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                    let start = Instant::now();
                    query(&retained)?;
                    let elapsed = milliseconds(start);
                    maximum = maximum.max(elapsed);
                    if samples.len() < 4096 {
                        samples.push(elapsed);
                    }
                    count += 1;
                }
                let held = if retain_old {
                    Some(retained)
                } else {
                    drop(retained);
                    None
                };
                let start = Instant::now();
                anyhow::ensure!(reader.refresh()?, "Writer did not publish");
                let refresh = milliseconds(start);
                let old_released = old_rdf.upgrade().is_none();
                anyhow::ensure!(
                    old_released != retain_old,
                    "Unexpected old snapshot ownership"
                );
                let start = Instant::now();
                query(&reader.snapshot())?;
                let new_index = milliseconds(start);
                if let Some(old) = &held {
                    anyhow::ensure!(old.copy_graph(&id)? == graph, "Retained view changed");
                }
                emit(
                    json!({"kind":"reader_refreshed", "generation":reader.snapshot().info().generation,
                    "retained_generation":if retain_old {Some(retained_generation)} else {None},
                    "old_snapshot_released":old_released, "refresh_ms":refresh,
                    "new_index_query_ms":new_index, "queries_during_hold":count,
                    "sample_count":samples.len(), "query_p50_ms":if samples.is_empty() {0.0} else {percentile(&mut samples, 50)},
                    "query_p95_ms":if samples.is_empty() {0.0} else {percentile(&mut samples, 95)},
                    "query_max_ms":maximum, "memory":memory()}),
                )?;
                anyhow::ensure!(receive.recv()?? == "exit");
            }
        }
        "update" => {
            let triples: usize = args.get(2).context("Missing triple count")?.parse()?;
            let burst: usize = args.get(3).context("Missing burst length")?.parse()?;
            let mut writer = SnapshotWriter::open(root)?;
            let mut commits = Vec::new();
            for revision in 1..=burst {
                let mut update = writer.begin_update();
                update.add_from_bytes(
                    location(0),
                    source(0, triples, revision),
                    Some(RdfFormat::Turtle),
                )?;
                let start = Instant::now();
                update.commit()?;
                commits.push(milliseconds(start));
            }
            let mut update = writer.begin_update();
            update.add_alias("urn:bench:alias", "urn:graph:0")?;
            let start = Instant::now();
            update.commit()?;
            emit(
                json!({"kind":"updated", "commit_ms":commits, "metadata_commit_ms":milliseconds(start),
                "generation":writer.snapshot().info().generation, "memory":memory()}),
            )?;
        }
        _ => anyhow::bail!("Unknown mode {mode}"),
    }
    Ok(())
}
