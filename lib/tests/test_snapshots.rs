use anyhow::{Context, Result};
use ontoenv::api::OntoEnv;
use ontoenv::config::Config;
use ontoenv::ontology::{GraphIdentifier, OntologyLocation};
use ontoenv::snapshots::{EnvironmentSnapshot, SnapshotReader, SnapshotWriter};
use oxigraph::io::RdfFormat;
use oxigraph::model::{Graph, NamedNode};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
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
fn snapshot_catalog_queries_retain_aliases_closures_and_namespaces() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let mut tx = writer.begin_update();
    let b = tx.add_from_bytes(source("urn:b"), ttl("urn:b", "b"), Some(RdfFormat::Turtle))?;
    tx.add_alias("urn:alias", "urn:b")?;
    let a = tx.add_from_bytes(source("urn:a"), b"@prefix chosen: <urn:source:> . <urn:a> a <http://www.w3.org/2002/07/owl#Ontology>; <http://www.w3.org/2002/07/owl#imports> <urn:alias> .".to_vec(), Some(RdfFormat::Turtle))?;
    tx.config_mut()
        .namespaces
        .insert("chosen".into(), "urn:bound:".into());
    let old = tx.commit()?;
    assert_eq!(old.get_closure(&a, -1)?, vec![a.clone(), b.clone()]);
    assert_eq!(old.get_closure(&a, 0)?, vec![a.clone()]);
    let alias = NamedNode::new("urn:alias")?;
    assert_eq!(old.get_importers(&alias)?, vec![a.clone()]);
    assert_eq!(
        old.get_import_paths(&alias)?,
        vec![vec![a.clone(), b.clone()]]
    );
    assert_eq!(old.get_namespaces(&a, true)?["chosen"], "urn:bound:");
    let mut tx = writer.begin_update();
    tx.remove_alias("urn:alias")?;
    tx.replace_graph(&a, old.get_graph(&b)?)
        .expect_err("different canonical IRI must be rejected");
    tx.replace_graph(&a, Graph::new())?;
    tx.config_mut()
        .namespaces
        .insert("chosen".into(), "urn:new:".into());
    let current = tx.commit()?;
    assert_eq!(current.get_closure(&a, -1)?, vec![a.clone()]);
    assert_eq!(current.get_namespaces(&a, false)?["chosen"], "urn:new:");
    assert_eq!(old.get_closure(&a, -1)?, vec![a.clone(), b.clone()]);
    assert_eq!(old.get_import_paths(&alias)?, vec![vec![a, b]]);
    Ok(())
}

#[test]
fn strict_closure_checks_only_imports_within_requested_depth() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut cfg = config(dir.path());
    cfg.strict = false;
    let mut writer = SnapshotWriter::create(cfg.clone())?;
    let mut tx = writer.begin_update();
    let b = tx.add_from_bytes(source("urn:b"), b"<urn:b> a <http://www.w3.org/2002/07/owl#Ontology>; <http://www.w3.org/2002/07/owl#imports> <urn:missing> .".to_vec(), Some(RdfFormat::Turtle))?;
    let a = tx.add_from_bytes(source("urn:a"), b"<urn:a> a <http://www.w3.org/2002/07/owl#Ontology>; <http://www.w3.org/2002/07/owl#imports> <urn:b> .".to_vec(), Some(RdfFormat::Turtle))?;
    let permissive = tx.commit()?;
    assert_eq!(permissive.get_closure(&a, -1)?, vec![a.clone(), b.clone()]);
    cfg.strict = true;
    let strict = writer.publish_config(cfg)?;
    assert_eq!(strict.get_closure(&a, 0)?, vec![a.clone()]);
    assert_eq!(strict.get_closure(&a, 1)?, vec![a.clone(), b.clone()]);
    assert!(strict
        .get_closure(&a, 2)
        .unwrap_err()
        .to_string()
        .contains("urn:missing"));
    assert!(
        matches!(strict.explain_import(&NamedNode::new("urn:missing")?), ontoenv::api::ImportPaths::Missing { importers } if importers == vec![vec![a.clone(), b.clone()]])
    );
    let mut live = OntoEnv::open(dir.path().to_path_buf(), true)?;
    assert_eq!(live.get_closure(&a, 1)?, strict.get_closure(&a, 1)?);
    assert!(live.get_closure(&a, 2).is_err());
    live.apply_config_overrides(&ontoenv::config::ConfigOverrides {
        strict: Some(false),
        ..Default::default()
    })?;
    assert_eq!(live.get_closure(&a, 2)?, vec![a.clone(), b]);
    assert!(live.snapshot().unwrap().get_closure(&a, 2).is_err());
    Ok(())
}

#[test]
fn cyclic_import_paths_are_finite_and_keep_resolved_aliases() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let mut tx = writer.begin_update();
    let a = tx.add_from_bytes(source("urn:a"), b"<urn:a> a <http://www.w3.org/2002/07/owl#Ontology>; <http://www.w3.org/2002/07/owl#imports> <urn:alias-b> .".to_vec(), Some(RdfFormat::Turtle))?;
    let b = tx.add_from_bytes(source("urn:b"), b"<urn:b> a <http://www.w3.org/2002/07/owl#Ontology>; <http://www.w3.org/2002/07/owl#imports> <urn:a> .".to_vec(), Some(RdfFormat::Turtle))?;
    tx.add_alias("urn:alias-b", "urn:b")?;
    let snapshot = tx.commit()?;
    assert_eq!(snapshot.get_closure(&a, -1)?, vec![a.clone(), b.clone()]);
    assert_eq!(
        snapshot.get_import_paths(&NamedNode::new("urn:alias-b")?)?,
        vec![vec![a, b]]
    );
    Ok(())
}

#[test]
fn candidate_queries_include_staged_graphs_and_aliases_and_recover_after_abort() -> Result<()> {
    use ontoenv::options::{Overwrite, RefreshStrategy};
    let dir = tempfile::tempdir()?;
    let mut env = OntoEnv::create(config(dir.path()), false)?;
    let result: Result<()> = env.transaction(|env| {
        let b = env.add_from_bytes_no_imports(source("urn:b"), ttl("urn:b", "b"), Some(RdfFormat::Turtle), Overwrite::Allow, RefreshStrategy::UseCache)?;
        env.add_alias("urn:alias", "urn:b")?;
        let a = env.add_from_bytes_no_imports(source("urn:a"), b"<urn:a> a <http://www.w3.org/2002/07/owl#Ontology>; <http://www.w3.org/2002/07/owl#imports> <urn:alias> .".to_vec(), Some(RdfFormat::Turtle), Overwrite::Allow, RefreshStrategy::UseCache)?;
        assert_eq!(env.get_closure(&a, -1)?, vec![a.clone(), b]);
        assert_eq!(env.get_importers(&NamedNode::new("urn:alias")?)?, vec![a]);
        anyhow::bail!("abort candidate")
    });
    assert!(result.is_err());
    assert!(env.ontologies().is_empty());
    assert!(env.snapshot().unwrap().ontologies().is_empty());
    assert!(env.get_importers(&NamedNode::new("urn:alias")?).is_err());
    Ok(())
}

#[test]
fn dataset_unions_keep_contexts_and_graph_projections_deduplicate() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let mut tx = writer.begin_update();
    let shared = "<urn:shared> <urn:p> <urn:object> .";
    let a = tx.add_from_bytes(
        source("urn:a"),
        format!("<urn:a> a <http://www.w3.org/2002/07/owl#Ontology> . {shared}").into_bytes(),
        Some(RdfFormat::Turtle),
    )?;
    let b = tx.add_from_bytes(
        source("urn:b"),
        format!("<urn:b> a <http://www.w3.org/2002/07/owl#Ontology> . {shared}").into_bytes(),
        Some(RdfFormat::Turtle),
    )?;
    let empty = tx.add_from_bytes(
        source("urn:empty"),
        ttl("urn:empty", "empty"),
        Some(RdfFormat::Turtle),
    )?;
    tx.replace_graph(&empty, Graph::new())?;
    let snapshot = tx.commit()?;
    let ids = [a.clone(), b, a, empty.clone()];
    let dataset = snapshot.copy_dataset(&ids)?;
    let graph = snapshot.copy_union(&ids)?;
    assert_eq!(dataset.len(), 4);
    assert_eq!(graph.len(), 3);
    assert_eq!(snapshot.graph_view(&empty)?.triple_count()?, 0);
    assert!(snapshot.copy_dataset(&[])?.is_empty());
    assert!(snapshot.copy_union(&[])?.is_empty());
    let missing = GraphIdentifier::new_with_location(
        NamedNode::new("urn:missing")?.as_ref(),
        source("urn:missing"),
    );
    assert!(snapshot.copy_dataset(&[empty, missing]).is_err());
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
fn cloned_handles_and_metadata_refresh_share_the_rdf_snapshot() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = OntoEnv::create_snapshot(config(dir.path()))?;
    let id = add(&mut writer, "urn:a", "old")?;
    let mut reader = OntoEnv::open_snapshot(dir.path())?;
    let old = reader.snapshot();
    let cloned = reader.snapshot();
    assert!(Arc::ptr_eq(&old, &cloned));
    let rdf = old.rdf();
    assert!(Arc::ptr_eq(&rdf, &cloned.rdf()));
    assert_eq!(
        rdf.grouped_scan(rdf5d::Pattern::ANY, rdf5d::Scope::All)?
            .count(),
        2
    );
    assert!(!reader.refresh()?);
    assert!(Arc::ptr_eq(&old, &reader.snapshot()));
    let old_metadata = Arc::downgrade(&old);
    let mut update = writer.begin_update();
    update.add_alias("urn:new-alias", "urn:a")?;
    update.commit()?;
    assert!(reader.refresh()?);
    assert!(!Arc::ptr_eq(&old, &reader.snapshot()));
    assert!(Arc::ptr_eq(&rdf, &reader.snapshot().rdf()));
    assert!(old
        .resolve(NamedNode::new("urn:new-alias")?.as_ref())
        .is_none());
    assert_eq!(
        reader
            .snapshot()
            .resolve(NamedNode::new("urn:new-alias")?.as_ref()),
        Some(id)
    );
    drop(old);
    drop(cloned);
    assert!(old_metadata.upgrade().is_none());
    Ok(())
}

#[test]
fn ordinary_refresh_releases_unreferenced_metadata_and_indexes() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let id = add(&mut writer, "urn:a", "old")?;
    let mut reader = SnapshotReader::open(dir.path())?;
    let old = reader.snapshot();
    let metadata = Arc::downgrade(&old);
    let rdf = old.rdf();
    let indexes = Arc::downgrade(&rdf);
    assert_eq!(
        rdf.grouped_scan(rdf5d::Pattern::ANY, rdf5d::Scope::All)?
            .count(),
        2
    );
    drop(rdf);
    drop(old);
    add(&mut writer, "urn:a", "new")?;
    assert!(reader.refresh()?);
    assert!(metadata.upgrade().is_none());
    assert!(indexes.upgrade().is_none());
    assert_value(&reader.snapshot(), &id, "new")?;
    Ok(())
}

#[test]
fn owned_query_iterator_pins_only_its_original_rdf_until_dropped() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    add(&mut writer, "urn:a", "old")?;
    let mut reader = SnapshotReader::open(dir.path())?;
    let old = reader.snapshot();
    let metadata = Arc::downgrade(&old);
    let rdf = old.rdf();
    let indexes = Arc::downgrade(&rdf);
    let p = rdf
        .file()
        .term_id(&rdf5d::DecodedTerm::Iri("urn:value".into()))
        .unwrap();
    let mut rows = rdf.grouped_scan(
        rdf5d::Pattern {
            p: Some(p),
            ..rdf5d::Pattern::ANY
        },
        rdf5d::Scope::All,
    )?;
    drop(rdf);
    drop(old);
    add(&mut writer, "urn:a", "new")?;
    reader.refresh()?;
    assert!(metadata.upgrade().is_none());
    let row = rows.next().context("missing old query result")?;
    let decoder = indexes.upgrade().context("iterator lost its backing")?;
    assert!(
        matches!(decoder.file().decoded_term(row.o)?, rdf5d::DecodedTerm::Literal { lex, .. } if lex == "old")
    );
    drop(decoder);
    drop(rows);
    assert!(indexes.upgrade().is_none());
    Ok(())
}

#[test]
fn normal_api_scoped_views_keep_the_selected_environment_version() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = OntoEnv::create_snapshot(config(dir.path()))?;
    let id = add(&mut writer, "urn:a", "old")?;
    let reader = OntoEnv::open_snapshot(dir.path())?;
    let snapshot = reader.snapshot();
    let old = Arc::downgrade(&snapshot);
    let view = snapshot.graph_view(&id)?;
    assert_eq!(view.triple_count()?, 2);
    let p = snapshot
        .rdf()
        .file()
        .term_id(&rdf5d::DecodedTerm::Iri("urn:value".into()))
        .unwrap();
    let pattern = rdf5d::Pattern {
        p: Some(p),
        ..rdf5d::Pattern::ANY
    };
    assert_eq!(
        view.scan(pattern)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .len(),
        1
    );
    drop(reader);
    drop(writer);
    let mut writer = OntoEnv::open_snapshot_writer(dir.path())?;
    let mut update = writer.begin_update();
    update.replace_graph(&id, Graph::new())?;
    update.commit()?;
    assert_eq!(view.triple_count()?, 2);
    assert_eq!(
        view.scan(pattern)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .len(),
        1
    );
    assert_eq!(writer.snapshot().graph_view(&id)?.triple_count()?, 0);
    let mut update = writer.begin_update();
    update.remove(&id)?;
    update.commit()?;
    assert!(writer.snapshot().graph_view(&id).is_err());
    drop(view);
    drop(snapshot);
    assert!(old.upgrade().is_none());
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
fn parsed_staging_preserves_sets_blank_nodes_and_literal_lexical_forms() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let mut tx = writer.begin_update();
    let bytes = br#"
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        <urn:duplicates> a owl:Ontology; <urn:node> _:shared .
        _:shared <urn:text> "salut"@fr;
            <urn:number> "01"^^xsd:integer, "1"^^xsd:integer .
        <urn:duplicates> <urn:node> _:other .
        _:other <urn:text> "salut"@fr .
        # Repeated, nonadjacent triples remain set members, not extra rows.
        <urn:duplicates> <urn:node> _:shared .
        _:shared <urn:text> "salut"@fr .
    "#;
    let id = tx.add_from_bytes(
        source("duplicates"),
        bytes.to_vec(),
        Some(RdfFormat::Turtle),
    )?;
    let staged = tx.get_graph(&id)?;
    assert_eq!(staged.len(), 7);
    let mut copy = tx.get_graph(&id)?;
    copy.clear();
    assert_eq!(tx.get_graph(&id)?.len(), 7);
    let committed = tx.commit()?;
    assert_eq!(committed.get_graph(&id)?, staged);
    assert_eq!(
        committed
            .rdf()
            .triple_count(rdf5d::Scope::ByName("urn:duplicates"))?,
        7
    );
    drop(writer);
    let reader = SnapshotReader::open(dir.path())?;
    assert_eq!(reader.snapshot().get_graph(&id)?, staged);
    Ok(())
}

#[test]
fn indexed_replacement_and_parsed_replacement_share_transaction_behavior() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    let id = add(&mut writer, "urn:a", "old")?;
    let retained = writer.snapshot();
    let mut tx = writer.begin_update();
    let graph = tx.get_graph(&id)?;
    tx.replace_graph(&id, graph.clone())?;
    assert_eq!(tx.get_graph(&id)?, graph);
    tx.add_alias("urn:alias", "urn:a")?;
    tx.add_from_bytes(
        source("urn:a"),
        ttl("urn:a", "new"),
        Some(RdfFormat::Turtle),
    )?;
    let parsed = tx.get_graph(&id)?;
    let committed = tx.commit()?;
    assert_eq!(committed.get_graph(&id)?, parsed);
    assert_eq!(
        committed.resolve(NamedNode::new("urn:alias")?.as_ref()),
        Some(id.clone())
    );
    assert_value(&retained, &id, "old")?;
    assert_value(&committed, &id, "new")?;
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
        1
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

fn publish_revision(writer: &mut SnapshotWriter, revision: u64) -> Result<()> {
    let mut update = writer.begin_update();
    update.add_from_bytes(
        source("urn:a"),
        ttl("urn:a", &revision.to_string()),
        Some(RdfFormat::Turtle),
    )?;
    update
        .config_mut()
        .namespaces
        .insert("revision".into(), format!("urn:revision:{revision}:"));
    update.add_alias(&format!("urn:revision:{revision}"), "urn:a")?;
    let snapshot = update.commit()?;
    assert_eq!(snapshot.info().generation, revision);
    Ok(())
}

fn assert_coherent_revision(snapshot: &EnvironmentSnapshot) -> Result<()> {
    let revision = snapshot.info().generation;
    let id = snapshot
        .resolve(NamedNode::new("urn:a")?.as_ref())
        .context("missing ontology")?;
    assert_value(snapshot, &id, &revision.to_string())?;
    assert_eq!(
        snapshot.config().namespaces["revision"],
        format!("urn:revision:{revision}:")
    );
    assert_eq!(
        snapshot.resolve(NamedNode::new(format!("urn:revision:{revision}"))?.as_ref()),
        Some(id)
    );
    Ok(())
}

#[test]
#[ignore = "child process refresh stress helper"]
fn snapshot_refresh_worker() -> Result<()> {
    let root = std::path::PathBuf::from(std::env::var("ONTOENV_SNAPSHOT_TEST_ROOT")?);
    let worker = std::env::var("ONTOENV_SNAPSHOT_TEST_WORKER")?;
    let mut reader = SnapshotReader::open(&root)?;
    let retained = reader.snapshot();
    assert_eq!(retained.info().generation, 1);
    assert_coherent_revision(&retained)?;
    fs::write(root.join(format!("reader-ready-{worker}")), b"ready")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut previous = 1;
    loop {
        let done = root.join("writer-done").exists();
        let fresh = SnapshotReader::open(&root)?;
        assert_coherent_revision(&fresh.snapshot())?;
        reader.refresh()?;
        let current = reader.snapshot();
        assert!(current.info().generation >= previous);
        previous = current.info().generation;
        assert_coherent_revision(&current)?;
        assert_coherent_revision(&retained)?;
        if done {
            assert_eq!(current.info().generation, 21);
            break;
        }
        anyhow::ensure!(Instant::now() < deadline, "writer did not finish");
        thread::yield_now();
    }
    Ok(())
}

#[test]
fn multiple_processes_open_and_refresh_during_repeated_publications() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = SnapshotWriter::create(config(dir.path()))?;
    publish_revision(&mut writer, 1)?;
    // Kill and reap helpers on early failure as well as normal completion.
    struct Workers(Vec<std::process::Child>);
    impl Drop for Workers {
        fn drop(&mut self) {
            for child in &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    let mut workers = Workers(Vec::new());
    for worker in 0..3 {
        workers.0.push(
            Command::new(std::env::current_exe()?)
                .args([
                    "--ignored",
                    "--exact",
                    "snapshot_refresh_worker",
                    "--nocapture",
                ])
                .env("ONTOENV_SNAPSHOT_TEST_ROOT", dir.path())
                .env("ONTOENV_SNAPSHOT_TEST_WORKER", worker.to_string())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?,
        );
    }
    for worker in 0..3 {
        wait_for(&dir.path().join(format!("reader-ready-{worker}")))?;
    }
    for revision in 2..=21 {
        publish_revision(&mut writer, revision)?;
    }
    fs::write(dir.path().join("writer-done"), b"done")?;
    while let Some(child) = workers.0.pop() {
        let output = child.wait_with_output()?;
        assert!(
            output.status.success(),
            "reader failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
