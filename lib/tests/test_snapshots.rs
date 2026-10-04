#![cfg(feature = "transactional-snapshots")]

use anyhow::{Context, Result};
use ontoenv::config::Config;
use ontoenv::ontology::{GraphIdentifier, OntologyLocation};
use ontoenv::snapshots::{EnvironmentSnapshot, SnapshotReader, SnapshotWriter};
use oxigraph::io::RdfFormat;
use oxigraph::model::{Graph, NamedNode};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn config(root: &Path) -> Config {
    Config::builder()
        .root(root.to_path_buf())
        .temporary(false)
        .strict(true)
        .offline(true)
        .locations(vec![])
        .build()
        .unwrap()
}

fn source(name: &str) -> OntologyLocation {
    OntologyLocation::InMemory {
        identifier: name.to_owned(),
    }
}

fn ttl(name: &str, value: &str) -> Vec<u8> {
    format!("<{name}> a <http://www.w3.org/2002/07/owl#Ontology> ; <urn:value> \"{value}\" .")
        .into_bytes()
}

fn add(writer: &mut SnapshotWriter, name: &str, value: &str) -> Result<GraphIdentifier> {
    let mut tx = writer.begin_update();
    let id = tx.add_from_bytes(source(name), ttl(name, value), Some(RdfFormat::Turtle))?;
    tx.commit()?;
    Ok(id)
}

fn assert_value(snapshot: &EnvironmentSnapshot, id: &GraphIdentifier, value: &str) -> Result<()> {
    let graph = snapshot.get_graph(id)?;
    assert!(graph
        .iter()
        .any(|triple| triple.object.to_string() == format!("\"{value}\"")));
    Ok(())
}

#[test]
fn readers_pin_generations_while_writer_remains_open() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let id = add(&mut writer, "urn:a", "old")?;
    let mut reader = SnapshotReader::open(dir.path())?;
    let retained = reader.snapshot();
    let retained_rdf = retained.rdf();
    assert_value(&retained, &id, "old")?;
    #[cfg(unix)]
    assert!(retained_rdf.file().is_mmap_backed());
    let next = add(&mut writer, "urn:a", "new")?;
    assert!(!reader.is_current()?);
    assert_value(&reader.snapshot(), &id, "old")?;
    assert!(reader.refresh()?);
    assert_value(&reader.snapshot(), &next, "new")?;
    assert_eq!(reader.snapshot().info().generation, 2);
    assert_eq!(reader.snapshot().info().parent_generation, Some(1));
    assert_value(&retained, &id, "old")?;
    assert_eq!(retained_rdf.triple_count(rdf5d::Scope::All)?, 2);
    assert!(!reader.refresh()?);
    Ok(())
}

#[test]
fn abort_and_failed_strict_commit_leave_no_partial_state() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let id = add(&mut writer, "urn:a", "old")?;
    let files_before = fs::read_dir(dir.path().join(".ontoenv-snapshots/objects"))?.count();
    {
        let mut tx = writer.begin_update();
        tx.add_from_bytes(
            source("urn:a"),
            ttl("urn:a", "uncommitted"),
            Some(RdfFormat::Turtle),
        )?;
        tx.config_mut().offline = false;
        tx.add_alias("urn:private", "urn:a")?;
        assert_eq!(tx.get_graph(&id)?.len(), 2);
        // Drop is rollback, even when previous staging operations succeeded.
    }
    assert_value(&writer.snapshot(), &id, "old")?;
    assert!(writer.snapshot().config().offline);
    assert!(writer
        .snapshot()
        .environment()
        .resolve_alias("urn:private")
        .is_none());
    let mut tx = writer.begin_update();
    let invalid = b"<urn:b> a <http://www.w3.org/2002/07/owl#Ontology> ; <http://www.w3.org/2002/07/owl#imports> <urn:missing> .";
    tx.add_from_bytes(source("urn:b"), invalid.to_vec(), Some(RdfFormat::Turtle))?;
    assert!(tx.commit().is_err());
    assert_eq!(writer.snapshot().info().generation, 1);
    assert_eq!(
        files_before,
        fs::read_dir(dir.path().join(".ontoenv-snapshots/objects"))?.count()
    );
    let reader = SnapshotReader::open(dir.path())?;
    assert_eq!(reader.snapshot().ontologies().len(), 1);
    assert_value(&reader.snapshot(), &id, "old")?;
    Ok(())
}

#[test]
fn empty_replacement_and_deletion_are_distinct_after_reopen() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let a = add(&mut writer, "urn:a", "a")?;
    let b = add(&mut writer, "urn:b", "b")?;
    let old = writer.snapshot();
    let mut tx = writer.begin_update();
    tx.replace_graph(&a, Graph::new())?;
    assert!(tx.remove(&b)?);
    tx.commit()?;
    drop(writer);
    let reader = SnapshotReader::open(dir.path())?;
    assert!(reader.snapshot().get_graph(&a)?.is_empty());
    assert!(reader.snapshot().get_graph(&b).is_err());
    assert_eq!(reader.snapshot().ontologies().len(), 1);
    assert_value(&old, &a, "a")?;
    assert_value(&old, &b, "b")?;
    Ok(())
}

#[test]
fn aliases_config_and_source_locations_survive_reopen() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let id = add(&mut writer, "urn:a", "old")?;
    let base = writer.snapshot();
    let mut tx = writer.begin_update();
    tx.add_alias("urn:alias", "urn:a")?;
    tx.config_mut()
        .namespaces
        .insert("test".to_owned(), "urn:test:".to_owned());
    let committed = tx.commit()?;
    // Catalog-only COW shares the large mapped backing instead of rewriting it.
    assert!(std::sync::Arc::ptr_eq(&base.rdf(), &committed.rdf()));
    assert_eq!(
        fs::read_dir(dir.path().join(".ontoenv-snapshots/objects"))?.count(),
        2
    );
    // Replacing the source location should preserve explicit aliases.
    let mut tx = writer.begin_update();
    let updated = tx.add_from_bytes(
        source("new-source"),
        ttl("urn:a", "new"),
        Some(RdfFormat::Turtle),
    )?;
    tx.commit()?;
    assert_ne!(id, updated);
    drop(writer);
    let reader = SnapshotReader::open(dir.path())?;
    let snapshot = reader.snapshot();
    assert_eq!(
        snapshot.resolve(NamedNode::new("urn:alias")?.as_ref()),
        Some(updated.clone())
    );
    assert_eq!(
        snapshot
            .environment()
            .graphid_from_location(&source("new-source")),
        Some(&updated)
    );
    assert_eq!(snapshot.config().namespaces["test"], "urn:test:");
    assert_value(&snapshot, &updated, "new")?;
    Ok(())
}

#[test]
fn writer_exclusion_and_recreation_use_a_stable_lock() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let writer = SnapshotWriter::create(config(dir.path()))?;
    assert!(SnapshotWriter::open(dir.path()).is_err());
    assert!(SnapshotWriter::create(config(dir.path())).is_err());
    let reader = SnapshotReader::open(dir.path())?;
    drop(writer);
    // Existing readers do not exclude a new writer.
    let writer = SnapshotWriter::open(dir.path())?;
    assert_eq!(
        reader.snapshot().info().environment_id,
        writer.snapshot().info().environment_id
    );
    drop(writer);
    assert!(SnapshotWriter::create(config(dir.path())).is_err());
    Ok(())
}

#[test]
fn failed_initialization_does_not_strand_an_environment() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut invalid = config(dir.path());
    invalid.resolution_policy = "no-such-policy".to_owned();
    assert!(SnapshotWriter::create(invalid).is_err());
    assert!(!dir.path().join(".ontoenv-snapshots").exists());
    let writer = SnapshotWriter::create(config(dir.path()))?;
    assert_eq!(writer.snapshot().info().generation, 0);
    Ok(())
}

#[test]
fn invalid_config_and_failed_parse_do_not_publish() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let id = add(&mut writer, "urn:a", "old")?;
    let mut tx = writer.begin_update();
    tx.config_mut().root = dir.path().join("different");
    assert!(tx.commit().is_err());
    let mut tx = writer.begin_update();
    assert!(tx
        .add_from_bytes(
            source("broken"),
            b"not rdf".to_vec(),
            Some(RdfFormat::Turtle)
        )
        .is_err());
    tx.abort();
    assert_value(&writer.snapshot(), &id, "old")?;
    assert_eq!(writer.snapshot().info().generation, 1);
    Ok(())
}

#[test]
fn changed_source_iri_removes_old_graph_and_preserves_explicit_aliases() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let old_id = add(&mut writer, "urn:a", "old")?;
    let mut tx = writer.begin_update();
    tx.add_alias("urn:alias", "urn:a")?;
    tx.commit()?;
    let mut tx = writer.begin_update();
    let new_id = tx.add_from_bytes(
        source("urn:a"),
        ttl("urn:renamed", "new"),
        Some(RdfFormat::Turtle),
    )?;
    tx.commit()?;
    let reader = SnapshotReader::open(dir.path())?;
    assert_eq!(reader.snapshot().ontologies().len(), 1);
    assert!(reader.snapshot().get_graph(&old_id).is_err());
    assert_eq!(
        reader
            .snapshot()
            .resolve(NamedNode::new("urn:alias")?.as_ref()),
        Some(new_id.clone())
    );
    assert_value(&reader.snapshot(), &new_id, "new")?;
    Ok(())
}

#[test]
fn failed_refresh_preserves_the_previous_generation() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let id = add(&mut writer, "urn:a", "old")?;
    let mut reader = SnapshotReader::open(dir.path())?;
    let paths_before: std::collections::HashSet<_> =
        fs::read_dir(dir.path().join(".ontoenv-snapshots/objects"))?
            .map(|entry| entry.unwrap().path())
            .collect();
    add(&mut writer, "urn:a", "new")?;
    let new_object = fs::read_dir(dir.path().join(".ontoenv-snapshots/objects"))?
        .map(|entry| entry.unwrap().path())
        .find(|path| !paths_before.contains(path))
        .unwrap();
    // Move the mapped file, never modify/truncate its inode in place.
    fs::rename(
        new_object.join("store.r5tu"),
        new_object.join("missing.r5tu"),
    )?;
    assert!(reader.refresh().is_err());
    assert_eq!(reader.snapshot().info().generation, 1);
    assert_value(&reader.snapshot(), &id, "old")?;
    fs::rename(
        new_object.join("missing.r5tu"),
        new_object.join("store.r5tu"),
    )?;
    assert!(reader.refresh()?);
    assert_value(&reader.snapshot(), &id, "new")?;
    Ok(())
}

#[test]
fn unchanged_graphs_keep_blank_nodes_and_literal_types() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let mut tx = writer.begin_update();
    let bytes = b"<urn:a> a <http://www.w3.org/2002/07/owl#Ontology> ; <urn:p> _:shared . _:shared <urn:q> \"hello\"@en ; <urn:r> \"42\"^^<http://www.w3.org/2001/XMLSchema#integer> .";
    let id = tx.add_from_bytes(source("urn:a"), bytes.to_vec(), Some(RdfFormat::Turtle))?;
    tx.commit()?;
    let before = writer.snapshot().get_graph(&id)?;
    add(&mut writer, "urn:b", "new")?;
    let after = writer.snapshot().get_graph(&id)?;
    assert_eq!(before, after);
    let tx = writer.begin_update();
    assert_eq!(tx.get_graph(&id)?, before);
    tx.abort();
    Ok(())
}

fn wait_for(path: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        anyhow::ensure!(
            Instant::now() < deadline,
            "Timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[test]
#[ignore = "child process helper"]
fn snapshot_reader_worker() -> Result<()> {
    let root = std::path::PathBuf::from(std::env::var("ONTOENV_SNAPSHOT_TEST_ROOT")?);
    let mut reader = SnapshotReader::open(&root)?;
    let old = reader.snapshot();
    let id = old
        .resolve(NamedNode::new("urn:a")?.as_ref())
        .context("resolve old graph")?;
    assert_value(&old, &id, "old")?;
    fs::write(root.join("reader-ready"), b"ready")?;
    wait_for(&root.join("writer-published"))?;
    assert_value(&reader.snapshot(), &id, "old")?;
    assert!(!reader.is_current()?);
    assert!(reader.refresh()?);
    assert_value(&reader.snapshot(), &id, "new")?;
    assert_value(&old, &id, "old")?;
    Ok(())
}

#[test]
fn cross_process_reader_opens_and_refreshes_with_live_writer() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    add(&mut writer, "urn:a", "old")?;
    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--ignored",
            "--exact",
            "snapshot_reader_worker",
            "--nocapture",
        ])
        .env("ONTOENV_SNAPSHOT_TEST_ROOT", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Err(error) = wait_for(&dir.path().join("reader-ready")) {
        let _ = child.kill();
        let output = child.wait_with_output()?;
        anyhow::bail!(
            "{error}; child stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    add(&mut writer, "urn:a", "new")?;
    fs::write(dir.path().join("writer-published"), b"published")?;
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "child failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
