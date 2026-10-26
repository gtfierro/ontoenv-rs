// Diagnostic audit probes: these assert observed bugs, NOT desired behavior.
// To reproduce: copy to lib/tests/state_review_probe.rs, then run
// cargo test -p ontoenv --test state_review_probe -- --nocapture
// Remove the temporary integration-test copy afterward.
use ontoenv::api::OntoEnv;
use ontoenv::config::Config;
use ontoenv::io::{GraphIO, PersistentGraphIO, ReadOnlyPersistentGraphIO};
use ontoenv::ontology::{GraphIdentifier, OntologyLocation};
use ontoenv::options::{Overwrite, RefreshStrategy};
use oxigraph::model::{Graph, NamedNode, Triple};

fn config(root: &std::path::Path) -> Config {
    Config::builder().root(root.to_path_buf()).offline(true)
        .temporary(false).locations(vec![]).strict(true).build().unwrap()
}

#[test]
fn probe_empty_replacement_resurrects_old_graph() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let id = GraphIdentifier::new(NamedNode::new("urn:graph")?.as_ref());
    let mut graph = Graph::new();
    graph.insert(Triple::new(NamedNode::new("urn:s")?, NamedNode::new("urn:p")?, NamedNode::new("urn:o")?).as_ref());
    let mut io = PersistentGraphIO::new(dir.path().to_path_buf(), true, false)?;
    io.add_named_graph(id.clone(), graph)?;
    drop(io);
    let mut io = PersistentGraphIO::new(dir.path().to_path_buf(), true, false)?;
    io.add_named_graph(id.clone(), Graph::new())?;
    assert!(io.get_graph(&id)?.is_empty());
    let reopened = ReadOnlyPersistentGraphIO::new(dir.path().to_path_buf(), true)?;
    assert_eq!(reopened.get_graph(&id)?.len(), 1, "audit: stale graph was resurrected on disk");
    Ok(())
}

#[test]
fn probe_alias_lost_on_reopen_and_readonly_can_save() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let path = root.join("a.ttl");
    std::fs::write(&path, "<urn:a> a <http://www.w3.org/2002/07/owl#Ontology> .")?;
    let mut env = OntoEnv::init(config(root), false)?;
    env.add_no_imports(OntologyLocation::File(path), Overwrite::Allow, RefreshStrategy::Force)?;
    env.add_alias("urn:alias", "urn:a")?;
    assert!(env.resolve_alias("urn:alias").is_some());
    drop(env);
    let mut env = OntoEnv::load_from_directory(root.to_path_buf(), true)?;
    assert!(env.resolve_alias("urn:alias").is_none(), "audit: alias lost at open");
    env.add_alias("urn:readonly-write", "urn:a")?;
    let (catalog, _, _) = ontoenv::catalog::load(&root.join(".ontoenv/catalog.r5tu"))?;
    assert!(catalog.resolve_alias("urn:readonly-write").is_some(), "audit: readonly handle wrote catalog");
    Ok(())
}

#[test]
fn probe_failed_import_persists_partial_graph() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let path = root.join("a.ttl");
    let missing = root.join("missing.ttl");
    std::fs::write(&path, format!("<urn:a> a <http://www.w3.org/2002/07/owl#Ontology> ; <http://www.w3.org/2002/07/owl#imports> <file://{}> .", missing.display()))?;
    let mut env = OntoEnv::init(config(root), false)?;
    assert!(env.add(OntologyLocation::File(path), Overwrite::Allow, RefreshStrategy::Force).is_err());
    assert!(root.join(".ontoenv/catalog.pending").exists());
    drop(env);
    let io = ReadOnlyPersistentGraphIO::new(root.join(".ontoenv"), true)?;
    assert_eq!(io.graph_ids()?.len(), 1, "audit: failed operation still committed graph");
    assert!(OntoEnv::load_from_directory(root.to_path_buf(), true).is_err());
    Ok(())
}

#[test]
fn probe_recreate_bypasses_live_writer_lock() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let first = OntoEnv::init(config(dir.path()), false)?;
    let second = OntoEnv::init(config(dir.path()), true)?;
    // Both handles claim exclusive ownership, but refer to different lock inodes.
    assert!(first.store_path().is_some());
    assert!(second.store_path().is_some());
    Ok(())
}
