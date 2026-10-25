use anyhow::Result;
use fs2::FileExt;
use ontoenv::api::{ConnectSync, OntoEnv, ResolveTarget};
use ontoenv::config::Config;
use ontoenv::environment::Environment;
use ontoenv::io::{GraphIO, PersistentGraphIO};
use ontoenv::ontology::OntologyLocation;
use ontoenv::options::{Overwrite, RefreshStrategy};
use oxigraph::io::RdfFormat;
use oxigraph::model::NamedNode;
use std::collections::BTreeMap;
use std::path::Path;

fn config(root: &Path) -> Config {
    Config::builder()
        .root(root.to_path_buf())
        .locations(vec![])
        .temporary(false)
        .offline(true)
        .strict(true)
        .require_ontology_names(true)
        .build()
        .unwrap()
}
fn ttl(name: &str, value: &str) -> Vec<u8> {
    format!("<{name}> a <http://www.w3.org/2002/07/owl#Ontology>; <urn:value> \"{value}\" .")
        .into_bytes()
}
fn add(env: &mut OntoEnv, name: &str, value: &str) -> Result<()> {
    env.add_from_bytes(
        OntologyLocation::InMemory {
            identifier: name.into(),
        },
        ttl(name, value),
        Some(RdfFormat::Turtle),
        Overwrite::Allow,
        RefreshStrategy::Force,
    )?;
    Ok(())
}
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            (
                e.path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}
fn legacy_fixture(root: &Path) -> Result<()> {
    let legacy = root.join(".ontoenv");
    std::fs::create_dir_all(&legacy)?;
    config(root).save_to_file(&legacy.join("ontoenv.json"))?;
    std::fs::File::create(legacy.join("store.lock"))?;
    let mut io = PersistentGraphIO::new(legacy.clone(), true, true)?;
    let ontology = io.add_from_bytes(
        OntologyLocation::InMemory {
            identifier: "urn:a".into(),
        },
        ttl("urn:a", "legacy"),
        Some(RdfFormat::Turtle),
        Overwrite::Allow,
    )?;
    let mut env = Environment::new();
    env.add_ontology(ontology)?;
    env.add_alias("urn:alias", "urn:a")?;
    io.flush()?;
    ontoenv::catalog::save(
        &legacy.join(ontoenv::catalog::CATALOG_FILE),
        &env,
        io.store_state()?,
        Default::default(),
    )?;
    Ok(())
}

#[test]
fn managed_iri_conflicts_follow_overwrite_instead_of_retaining_policy_candidates() -> Result<()> {
    for policy in ["default", "latest", "version"] {
        let dir = tempfile::tempdir()?;
        let mut cfg = config(dir.path());
        cfg.resolution_policy = policy.to_owned();
        let mut env = OntoEnv::connect(cfg, ConnectSync::Auto, false)?;
        let first = OntologyLocation::InMemory {
            identifier: "first-source".into(),
        };
        let second = OntologyLocation::InMemory {
            identifier: "second-source".into(),
        };
        let name = "urn:shared";
        let versioned = |value: &str, version: u32| {
            let mut bytes = ttl(name, value);
            bytes.extend_from_slice(
                format!(" <{name}> <http://www.w3.org/2002/07/owl#versionInfo> \"{version}\" .")
                    .as_bytes(),
            );
            bytes
        };
        let old = env.add_from_bytes(
            first.clone(),
            versioned("first", 2),
            Some(RdfFormat::Turtle),
            Overwrite::Allow,
            RefreshStrategy::Force,
        )?;
        let held = env.snapshot().unwrap();
        env.add_alias("urn:chosen", name)?;
        let preserved = env.add_from_bytes(
            second.clone(),
            versioned("second", 99),
            Some(RdfFormat::Turtle),
            Overwrite::Preserve,
            RefreshStrategy::Force,
        )?;
        assert_eq!(preserved, old);
        assert_eq!(env.ontologies().len(), 1);
        assert_eq!(env.get_ontology(&old)?.location(), Some(&first));
        let replacement = env.add_from_bytes(
            second.clone(),
            versioned("second", 1),
            Some(RdfFormat::Turtle),
            Overwrite::Allow,
            RefreshStrategy::Force,
        )?;
        assert_eq!(env.ontologies().len(), 1);
        assert_eq!(env.get_ontology(&replacement)?.location(), Some(&second));
        assert_eq!(env.resolve_alias("urn:chosen"), Some(replacement.clone()));
        assert!(env
            .get_graph(&replacement)?
            .iter()
            .any(|t| t.object.to_string() == "\"second\""));
        assert!(held
            .get_graph(&old)?
            .iter()
            .any(|t| t.object.to_string() == "\"first\""));
        drop(env);
        let reader = OntoEnv::open(dir.path().to_path_buf(), true)?;
        assert_eq!(reader.ontologies().len(), 1);
        assert_eq!(reader.get_ontology(&replacement)?.location(), Some(&second));
        assert_eq!(reader.resolve_alias("urn:chosen"), Some(replacement));
    }
    Ok(())
}

#[test]
fn default_storage_reader_refresh_and_old_views_are_coherent() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = OntoEnv::connect(config(dir.path()), ConnectSync::Auto, false)?;
    assert!(dir.path().join(".ontoenv-snapshots/CURRENT").exists());
    assert!(!dir.path().join(".ontoenv").exists());
    add(&mut writer, "urn:a", "old")?;
    let mut reader = OntoEnv::open(dir.path().to_path_buf(), true)?;
    let old = reader.snapshot().unwrap();
    let id = reader
        .resolve(ResolveTarget::Graph(NamedNode::new("urn:a")?))
        .unwrap();
    add(&mut writer, "urn:a", "new")?;
    writer.add_alias("urn:alias", "urn:a")?;
    assert!(reader.resolve_alias("urn:alias").is_none());
    assert!(reader.refresh()?);
    assert!(reader.resolve_alias("urn:alias").is_some());
    assert!(old
        .get_graph(&id)?
        .iter()
        .any(|t| t.object.to_string() == "\"old\""));
    assert!(reader
        .get_graph(&id)?
        .iter()
        .any(|t| t.object.to_string() == "\"new\""));
    let before = files(&dir.path().join(".ontoenv-snapshots"));
    assert!(reader.add_alias("urn:bad", "urn:a").is_err());
    reader.flush()?;
    assert_eq!(before, files(&dir.path().join(".ontoenv-snapshots")));
    Ok(())
}

#[test]
fn failed_recursive_import_rolls_back_graphs_metadata_and_head() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut env = OntoEnv::create(config(dir.path()), false)?;
    add(&mut env, "urn:existing", "ok")?;
    let before = env.snapshot().unwrap();
    let bytes=b"<urn:new> a <http://www.w3.org/2002/07/owl#Ontology>; <http://www.w3.org/2002/07/owl#imports> <https://missing.invalid/dependency> .";
    assert!(env
        .add_from_bytes(
            OntologyLocation::InMemory {
                identifier: "urn:new".into()
            },
            bytes.to_vec(),
            Some(RdfFormat::Turtle),
            Overwrite::Allow,
            RefreshStrategy::Force
        )
        .is_err());
    assert_eq!(
        env.snapshot().unwrap().info().generation,
        before.info().generation
    );
    assert_eq!(env.ontologies().len(), 1);
    assert!(env
        .io()
        .graph_ids()?
        .iter()
        .all(|id| id.name().as_str() == "urn:existing"));
    add(&mut env, "urn:retry", "ok")?;
    Ok(())
}

#[test]
fn migration_is_explicit_preserves_legacy_and_publishes_a_complete_generation() -> Result<()> {
    let dir = tempfile::tempdir()?;
    legacy_fixture(dir.path())?;
    let before = files(&dir.path().join(".ontoenv"));
    assert!(OntoEnv::connect(config(dir.path()), ConnectSync::Auto, false).is_err());
    let env = OntoEnv::migrate(dir.path())?;
    assert_eq!(env.snapshot().unwrap().info().generation, 0);
    assert_eq!(env.ontologies().len(), 1);
    let id = env.resolve_alias("urn:alias").unwrap();
    assert!(env
        .get_graph(&id)?
        .iter()
        .any(|t| t.object.to_string() == "\"legacy\""));
    assert_eq!(files(&dir.path().join(".ontoenv")), before);
    assert!(OntoEnv::migrate(dir.path()).is_err());
    assert_eq!(files(&dir.path().join(".ontoenv")), before);
    Ok(())
}

#[test]
fn migration_refuses_active_legacy_handles_and_interrupted_mutations() -> Result<()> {
    let dir = tempfile::tempdir()?;
    legacy_fixture(dir.path())?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.path().join(".ontoenv/store.lock"))?;
    FileExt::lock_exclusive(&lock)?;
    assert!(OntoEnv::migrate(dir.path()).is_err());
    assert!(!dir.path().join(".ontoenv-snapshots").exists());
    drop(lock);
    std::fs::write(dir.path().join(".ontoenv/catalog.pending"), b"interrupted")?;
    assert!(OntoEnv::migrate(dir.path()).is_err());
    assert!(!dir.path().join(".ontoenv-snapshots").exists());
    Ok(())
}

#[test]
fn migration_rejects_duplicate_canonical_iris_without_installing_or_changing_legacy() -> Result<()>
{
    let dir = tempfile::tempdir()?;
    legacy_fixture(dir.path())?;
    let legacy = dir.path().join(".ontoenv");
    let catalog = legacy.join(ontoenv::catalog::CATALOG_FILE);
    let (mut environment, _, _) = ontoenv::catalog::load(&catalog)?;
    let mut io = PersistentGraphIO::new(legacy.clone(), true, true)?;
    let duplicate = io.add_from_bytes(
        OntologyLocation::InMemory {
            identifier: "second-source-for-the-same-iri".into(),
        },
        ttl("urn:a", "second source"),
        Some(RdfFormat::Turtle),
        Overwrite::Allow,
    )?;
    environment.add_ontology(duplicate)?;
    assert_eq!(environment.ontologies().len(), 2);
    io.flush()?;
    ontoenv::catalog::save(
        &catalog,
        &environment,
        io.store_state()?,
        Default::default(),
    )?;
    drop(io);
    let before = files(&legacy);

    let error = OntoEnv::migrate(dir.path()).unwrap_err();
    assert!(
        error.to_string().contains("duplicate graph IRI"),
        "{error:#}"
    );
    assert!(!dir.path().join(".ontoenv-snapshots").exists());
    assert_eq!(files(&legacy), before);
    assert!(dir.path().read_dir()?.all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".ontoenv-snapshots-init-")));
    Ok(())
}

#[test]
fn nested_transactions_restore_overlay_and_settings_and_publish_once() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut env = OntoEnv::create(config(dir.path()), false)?;
    let base = env.snapshot().unwrap();
    env.transaction(|env| {
        add(env, "urn:first", "first")?;
        let failed: Result<()> = env.transaction(|env| {
            add(env, "urn:discard", "discard")?;
            env.set_offline(false);
            env.set_require_ontology_names(false);
            anyhow::bail!("rollback this nested operation");
        });
        assert!(failed.is_err());
        assert!(env.is_offline());
        assert!(env.io().is_offline());
        assert!(env.requires_ontology_names());
        assert_eq!(env.io().graph_ids()?.len(), 1);
        add(env, "urn:second", "second")?;
        env.add_alias("urn:alias", "urn:first")?;
        assert_eq!(
            env.snapshot().unwrap().info().generation,
            base.info().generation
        );
        Ok(())
    })?;
    assert_eq!(
        env.snapshot().unwrap().info().generation,
        base.info().generation + 1
    );
    assert_eq!(env.ontologies().len(), 2);
    let selected = env.snapshot().unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<()> = env.transaction(|env| {
            add(env, "urn:panic", "discard")?;
            env.set_offline(false);
            panic!("abort update during unwinding");
        });
    }));
    assert!(panic.is_err());
    assert!(env.io().is_offline());
    assert_eq!(
        env.snapshot().unwrap().info().generation,
        selected.info().generation
    );
    assert_eq!(env.ontologies().len(), 2);
    add(&mut env, "urn:retry", "ok")?;
    Ok(())
}

#[test]
fn nested_rollback_restores_parsed_contents_after_staged_reads_and_rename() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut env = OntoEnv::create(config(dir.path()), false)?;
    env.transaction(|env| {
        env.add_from_bytes(
            OntologyLocation::InMemory {
                identifier: "duplicates".into(),
            },
            b"<urn:a> a <http://www.w3.org/2002/07/owl#Ontology>; <urn:value> \"old\", \"old\" ."
                .to_vec(),
            Some(RdfFormat::Turtle),
            Overwrite::Allow,
            RefreshStrategy::Force,
        )?;
        let id = env
            .resolve(ResolveTarget::Graph(NamedNode::new("urn:a")?))
            .unwrap();
        let expected = env.get_graph(&id)?;
        assert_eq!(expected.len(), 2);
        let mut copy = env.get_graph(&id)?;
        copy.clear();
        assert_eq!(env.get_graph(&id)?, expected);
        let failed: Result<()> = env.transaction(|env| {
            add(env, "urn:a", "discard")?;
            let replaced = env
                .resolve(ResolveTarget::Graph(NamedNode::new("urn:a")?))
                .unwrap();
            assert!(env
                .get_graph(&replaced)?
                .iter()
                .any(|t| t.object.to_string() == "\"discard\""));
            env.rename_graph_iri(&replaced, NamedNode::new("urn:renamed")?)?;
            anyhow::bail!("restore parsed source after indexed rename");
        });
        assert!(failed.is_err());
        assert!(env
            .resolve(ResolveTarget::Graph(NamedNode::new("urn:renamed")?))
            .is_none());
        assert_eq!(env.get_graph(&id)?, expected);
        Ok(())
    })?;
    let selected = env.snapshot().unwrap();
    assert_eq!(selected.info().generation, 1);
    let id = selected.resolve(NamedNode::new("urn:a")?.as_ref()).unwrap();
    assert_eq!(selected.get_graph(&id)?.len(), 2);
    assert_eq!(selected.rdf().triple_count(rdf5d::Scope::All)?, 2);
    Ok(())
}

#[test]
fn logical_recreation_keeps_old_readers_and_views_valid() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut writer = OntoEnv::create(config(dir.path()), false)?;
    add(&mut writer, "urn:a", "old")?;
    let mut reader = OntoEnv::open(dir.path().to_path_buf(), true)?;
    let old = reader.snapshot().unwrap();
    let id = old.ontologies().keys().next().unwrap().clone();
    let old_view = old.graph_view(&id)?;
    drop(writer);
    let recreated = OntoEnv::create(config(dir.path()), true)?;
    assert!(recreated.ontologies().is_empty());
    assert_eq!(reader.ontologies().len(), 1);
    assert!(reader.refresh()?);
    assert!(reader.ontologies().is_empty());
    assert!(!old.get_graph(&id)?.is_empty());
    assert!(old_view.triple_count()? > 0);
    Ok(())
}

#[test]
fn copied_environment_uses_its_opened_root_for_future_publications() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let original = dir.path().join("original");
    let copied = dir.path().join("copied");
    let mut writer = OntoEnv::create(config(&original), false)?;
    add(&mut writer, "urn:a", "original")?;
    drop(writer);
    for entry in walkdir::WalkDir::new(&original) {
        let entry = entry?;
        let target = copied.join(entry.path().strip_prefix(&original)?);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    let original_files = files(&original);
    let mut writer = OntoEnv::open(copied.clone(), false)?;
    assert_eq!(
        writer.snapshot().unwrap().config().root,
        std::fs::canonicalize(&copied)?
    );
    add(&mut writer, "urn:b", "copied")?;
    assert_eq!(files(&original), original_files);
    assert_eq!(OntoEnv::open(copied, true)?.ontologies().len(), 2);
    Ok(())
}
