//! Run with `cargo run -p ontoenv --features transactional-snapshots
//! --example transactional_snapshots`.

use anyhow::Result;
use ontoenv::config::Config;
use ontoenv::ontology::OntologyLocation;
use ontoenv::snapshots::{SnapshotReader, SnapshotWriter};
use oxigraph::io::RdfFormat;
use oxigraph::model::NamedNode;

fn main() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let config = Config::builder()
        .root(directory.path().to_path_buf())
        .temporary(false)
        .strict(true)
        .offline(true)
        .locations(vec![])
        .build()?;
    let mut writer = SnapshotWriter::create(config)?;
    let source = OntologyLocation::InMemory {
        identifier: "example".to_owned(),
    };
    let mut update = writer.begin_update();
    let id = update.add_from_bytes(
        source.clone(),
        b"<urn:example> a <http://www.w3.org/2002/07/owl#Ontology> ; <urn:value> \"old\" ."
            .to_vec(),
        Some(RdfFormat::Turtle),
    )?;
    update.commit()?;

    // The reader opens even though the writer remains alive.
    let mut reader = SnapshotReader::open(directory.path())?;
    let old = reader.snapshot();
    let rdf = old.rdf();
    let old_view = rdf5d::View::from_names(&rdf, &["urn:example"]);

    let mut update = writer.begin_update();
    update.add_from_bytes(
        source,
        b"<urn:example> a <http://www.w3.org/2002/07/owl#Ontology> ; <urn:value> \"new\" ; <urn:extra> \"another triple\" .".to_vec(),
        Some(RdfFormat::Turtle),
    )?;
    update.add_alias("urn:alias", "urn:example")?;
    let committed = update.commit()?;

    assert_eq!(reader.snapshot().info().generation, 1);
    assert_eq!(reader.snapshot().get_graph(&id)?.len(), 2);
    assert!(!reader.is_current()?);
    reader.refresh()?;
    assert_eq!(reader.snapshot().get_graph(&id)?.len(), 3);
    assert_eq!(old_view.triple_count()?, 2);
    assert_eq!(old.get_graph(&id)?.len(), 2);
    assert!(reader
        .snapshot()
        .resolve(NamedNode::new("urn:alias")?.as_ref())
        .is_some());

    println!(
        "Reader refreshed from generation {} to {}",
        old.info().generation,
        committed.info().generation
    );
    println!(
        "Retained view: {} triples; current graph: {} triples",
        old_view.triple_count()?,
        reader.snapshot().get_graph(&id)?.len()
    );
    println!("RDF5D mmap backing: {}", rdf.file().is_mmap_backed());
    Ok(())
}
