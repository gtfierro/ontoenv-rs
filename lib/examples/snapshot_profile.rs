//! Diagnostic warm-open/update profile; not a cold-cache benchmark.
//! Arguments: number of graphs (default 100), triples per graph (default 500).

use anyhow::{Context, Result};
use ontoenv::config::Config;
use ontoenv::ontology::OntologyLocation;
use ontoenv::snapshots::{SnapshotReader, SnapshotWriter};
use oxigraph::io::RdfFormat;
use rdf5d::{R5tuFile, Snapshot};
use std::fmt::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn source(graph: usize, triples: usize, revision: usize) -> Vec<u8> {
    let mut turtle = format!("<urn:graph:{graph}> a <http://www.w3.org/2002/07/owl#Ontology> .\n");
    for triple in 0..triples {
        let mut literal = String::new();
        for part in 0..4 {
            let hash = blake3::hash(format!("{graph}:{triple}:{part}:{revision}").as_bytes());
            write!(literal, "{}", hash.to_hex()).unwrap();
        }
        writeln!(
            turtle,
            "<urn:subject:{graph}:{triple}> <urn:value> \"{literal}\" ."
        )
        .unwrap();
    }
    turtle.into_bytes()
}

fn average(mut operation: impl FnMut() -> Result<()>) -> Result<Duration> {
    let started = Instant::now();
    for _ in 0..10 {
        operation()?;
    }
    Ok(started.elapsed() / 10)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let graphs = args
        .first()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(100);
    let triples = args
        .get(1)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(500);
    anyhow::ensure!(graphs > 0 && triples > 0, "Counts must be positive");
    let directory = tempfile::tempdir()?;
    let config = Config::builder()
        .root(directory.path().to_path_buf())
        .temporary(false)
        .strict(true)
        .offline(true)
        .locations(vec![])
        .build()?;
    let mut writer = SnapshotWriter::create(config)?;
    let mut update = writer.begin_update();
    for graph in 0..graphs {
        update.add_from_bytes(
            OntologyLocation::InMemory {
                identifier: format!("graph-{graph}"),
            },
            source(graph, triples, 0),
            Some(RdfFormat::Turtle),
        )?;
    }
    let started = Instant::now();
    update.commit()?;
    let first_commit = started.elapsed();

    // This fresh profile has exactly an empty and a populated object. Select
    // the populated file solely to compare opening the same bytes both ways.
    let store = std::fs::read_dir(directory.path().join(".ontoenv-snapshots/objects"))?
        .map(|entry| entry.map(|entry| entry.path().join("store.r5tu")))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .max_by_key(|path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
        .context("No RDF5D object")?;
    let bytes = std::fs::metadata(&store)?.len();
    let direct_mmap = average(|| {
        std::hint::black_box(Snapshot::open(&store)?);
        Ok(())
    })?;
    let direct_owned = average(|| {
        std::hint::black_box(Snapshot::from_file(Arc::new(R5tuFile::open(&store)?))?);
        Ok(())
    })?;
    let manifest_open = average(|| {
        std::hint::black_box(SnapshotReader::open(directory.path())?);
        Ok(())
    })?;

    let mut reader = SnapshotReader::open(directory.path())?;
    let retained = reader.snapshot();
    let mut update = writer.begin_update();
    update.add_from_bytes(
        OntologyLocation::InMemory {
            identifier: "graph-0".to_owned(),
        },
        source(0, triples, 1),
        Some(RdfFormat::Turtle),
    )?;
    let started = Instant::now();
    update.commit()?;
    let rewrite_commit = started.elapsed();
    let started = Instant::now();
    reader.refresh()?;
    let refresh = started.elapsed();
    println!(
        "{graphs} graphs, {} triples, {:.2} MiB RDF5D",
        graphs * (triples + 1),
        bytes as f64 / 1048576.0
    );
    println!("serialization/first commit: {first_commit:?}");
    println!("warm direct RDF5D mmap open: {direct_mmap:?}");
    println!("warm direct RDF5D owned open: {direct_owned:?}");
    println!("warm manifest + RDF5D mmap open: {manifest_open:?}");
    println!("one-graph change / complete-file commit: {rewrite_commit:?}");
    println!(
        "reader refresh: {refresh:?}; retained generation: {}",
        retained.info().generation
    );
    Ok(())
}
