use anyhow::Result;
use ontoenv::api::{OntoEnv, ResolveTarget};
use ontoenv::config::{Config, HowCreated};
use ontoenv::ontology::OntologyLocation;
use oxigraph::model::NamedNodeRef;
use std::path::PathBuf;
use tempdir::TempDir;

// the tests directory contains a number of test files that are used to test the OntoEnv.
// Each has a unique name and they all exist in a flat folder.
// This is a macro which takes a list of strings describing the directory structure of a
// test directory and creates a temporary directory with the given structure. The strings
// in the test directory might be nested in different directories. The macro copies the
// files to the temporary directory and returns the temporary directory.
macro_rules! setup {
    ($temp_dir:expr, { $($from:expr => $to:expr),* $(,)? }) => {{
        use std::collections::HashSet;
        use std::path::PathBuf;
        use std::fs;

        // Assign the temporary directory
        let dir = $temp_dir;

        // Create a HashSet of the destination files
        let provided_files: HashSet<&str> = {
            let mut set = HashSet::new();
            $( set.insert($to); )*
            set
        };

        // Copy each specified file to the temporary directory
        $(
            let source_path: PathBuf = PathBuf::from($from);
            let dest_path: PathBuf = dir.path().join($to);
            if !dest_path.exists() {
                // Ensure the parent directories exist
                if let Some(parent) = dest_path.parent() {
                    if !parent.exists() {
                        fs::create_dir_all(parent).expect("Failed to create parent directories");
                    }
                }

                // 'copy_file' is assumed to be a custom function in the user's project
                // If not, consider using std::fs::copy for basic file copying
                copy_file(&source_path, &dest_path).expect(format!("Failed to copy file from {} to {}", source_path.display(), dest_path.display()).as_str());
            }
        )*

        // Check the contents of the temporary directory
        for entry in fs::read_dir(dir.path()).expect("Failed to read directory") {
            let entry = entry.expect("Failed to read entry");
            let file_name = entry.file_name().into_string().expect("Failed to convert filename to string");

            if !provided_files.contains(file_name.as_str()) && entry.file_type().expect("Failed to get file type").is_file() {
                // remove it
                fs::remove_file(entry.path()).expect("Failed to remove file");
            }
        }
    }};
}

fn copy_file(src_path: &PathBuf, dst_path: &PathBuf) -> Result<(), std::io::Error> {
    if let Some(parent) = dst_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src_path, dst_path)?;
    Ok(())
}

fn default_config(dir: &TempDir) -> Config {
    Config::new(
        dir.path().into(),
        Some(vec![dir.path().into()]),
        &["*.ttl", "*.xml"],
        &[""],
        false,
        true,
        true,
        "default".to_string(),
        false, // no search
        false, // temporary
    )
    .unwrap()
}

fn default_config_ttl_only(dir: &TempDir) -> Config {
    Config::new(
        dir.path().into(),
        Some(vec![dir.path().into()]),
        &["*.ttl"],
        &[""],
        false,
        true,
        true,
        "default".to_string(),
        false, // no search
        false, // temporary
    )
    .unwrap()
}

fn default_config_with_subdir(dir: &TempDir, path: &str) -> Config {
    Config::new(
        dir.path().into(),
        Some(vec![dir.path().join(path)]),
        &["*.ttl"],
        &[""],
        false,
        false,
        true,
        "default".to_string(),
        false, // no search
        false, // temporary
    )
    .unwrap()
}

// we don't care about errors when cleaning up the TempDir so
// we just drop the TempDir (looking at this doc:
// https://docs.rs/tempdir/latest/tempdir/struct.TempDir.html#method.close)
fn teardown(_dir: TempDir) {}

#[test]
fn test_ontoenv_scans() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont2.ttl" => "ont2.ttl",
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl" });
    // print the files in dir
    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 4);
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_scans_default() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont2.ttl" => "ont2.ttl",
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl" });
    let cfg = Config::new_with_default_matches(
        dir.path().into(),
        Some([dir.path().into()]),
        false,
        false,
        true,
        false, // no temporary
    )?;
    let mut env = OntoEnv::init(cfg, false)?;
    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 4);
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_num_triples() -> Result<()> {
    let dir = TempDir::new("fileendings")?;
    setup!(&dir, {"fixtures/fileendings/model" => "model", 
                  "fixtures/fileendings/model.n3" => "model.n3",
                  "fixtures/fileendings/model.nt" => "model.nt",
                  "fixtures/fileendings/model.ttl" => "model.ttl",
                  "fixtures/fileendings/model.xml" => "model.xml"});
    let cfg1 = Config::new(
        dir.path().into(),
        Some(vec![dir.path().into()]),
        &["*.n3"],
        &[""],
        false,
        false,
        true,
        "default".to_string(),
        false, // no search
        false, // no temporary
    )?;
    let mut env = OntoEnv::init(cfg1, false)?;
    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 1);
    assert_eq!(env.stats()?.num_triples, 5);
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_update() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont2.ttl" => "ont2.ttl",
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl" });
    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update()?;
    let old_num_triples = env.stats()?.num_triples;
    assert_eq!(env.stats()?.num_graphs, 4);

    // updating again shouldn't add anything
    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 4);
    assert_eq!(env.stats()?.num_triples, old_num_triples);

    // remove ont2.ttl
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl"});

    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 3);

    // copy ont4.ttl back
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont2.ttl" => "ont2.ttl",
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl" });
    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 4);

    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_retrieval_by_name() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont2.ttl" => "ont2.ttl",
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl" });
    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update()?;

    let ont1 = NamedNodeRef::new("urn:ont1")?;
    let ont_id = env
        .resolve(ResolveTarget::Graph(ont1.into()))
        .ok_or(anyhow::anyhow!("Ontology not found"))?;
    let ont = env.get_ontology(&ont_id)?;
    assert_eq!(ont.imports.len(), 1);
    assert!(ont.location().expect("should be a location").is_file());

    let ont2 = NamedNodeRef::new("urn:ont2")?;
    let ont_id = env
        .resolve(ResolveTarget::Graph(ont2.into()))
        .ok_or(anyhow::anyhow!("Ontology not found"))?;
    let ont = env.get_ontology(&ont_id)?;
    assert_eq!(ont.imports.len(), 2);
    assert!(ont.location().unwrap().is_file());
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_retrieval_by_location() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont2.ttl" => "ont2.ttl",
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl" });
    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update()?;

    let ont1_path = dir.path().join("ont1.ttl");
    let loc = OntologyLocation::from_str(
        ont1_path
            .to_str()
            .ok_or(anyhow::anyhow!("Failed to convert to string"))?,
    )?;
    let ont_id = env
        .resolve(ResolveTarget::Location(loc.clone()))
        .ok_or(anyhow::anyhow!("Ontology not found"))?;
    let ont = env.get_ontology(&ont_id)?;
    assert_eq!(ont.imports.len(), 1);
    assert!(ont
        .location()
        .ok_or(anyhow::anyhow!("Location not found"))?
        .is_file());
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_load() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont2.ttl" => "ont2.ttl",
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl" });
    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 4);
    env.save_to_directory()?;
    // drop env
    drop(env);

    // reload env
    let cfg_location = dir.path();
    let env2 = OntoEnv::load_from_directory(cfg_location.to_path_buf(), false)?;
    assert_eq!(env2.stats()?.num_graphs, 4);
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_add() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    setup!(&dir, {"fixtures/updates/v1/ont1.ttl" => "v1/ont1.ttl",
                  "fixtures/updates/v1/ont2.ttl" => "v1/ont2.ttl",
                  "fixtures/updates/v1/ont3.ttl" => "v1/ont3.ttl",
                  "fixtures/updates/v1/ont4.ttl" => "v1/ont4.ttl",
                  "fixtures/updates/v2/ont5.ttl" => "v2/ont5.ttl"
    });

    let cfg1 = default_config_with_subdir(&dir, "v1");
    let mut env = OntoEnv::init(cfg1, false)?;
    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 4);

    let ont_path = dir.path().join("v2/ont5.ttl");
    let loc = OntologyLocation::from_str(
        ont_path
            .to_str()
            .ok_or(anyhow::anyhow!("Failed to convert to string"))?,
    )?;
    env.add(loc, true)?;
    assert_eq!(env.stats()?.num_graphs, 5);
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_detect_updates() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    setup!(&dir, {"fixtures/updates/v1/ont1.ttl" => "v1/ont1.ttl",
                  "fixtures/updates/v1/ont2.ttl" => "v1/ont2.ttl",
                  "fixtures/updates/v1/ont3.ttl" => "v1/ont3.ttl",
                  "fixtures/updates/v1/ont4.ttl" => "v1/ont4.ttl",
    });
    let cfg1 = default_config_with_subdir(&dir, "v1");
    let mut env = OntoEnv::init(cfg1, false)?;
    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 4);

    // copy files from dir/v2 to dir/v1
    setup!(&dir, {"fixtures/updates/v1/ont1.ttl" => "v1/ont1.ttl",
                  "fixtures/updates/v1/ont2.ttl" => "v1/ont2.ttl",
                  "fixtures/updates/v1/ont4.ttl" => "v1/ont4.ttl",
                  "fixtures/updates/v2/ont3.ttl" => "v1/ont3.ttl",
                  "fixtures/updates/v2/ont5.ttl" => "v1/ont5.ttl",
    });
    env.update()?;

    assert_eq!(env.stats()?.num_graphs, 5);
    teardown(dir);
    Ok(())
}

#[test]
fn test_check_for_updates() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    let cfg1 = default_config_with_subdir(&dir, "v1");
    setup!(&dir, {"fixtures/updates/v1/ont1.ttl" => "v1/ont1.ttl",
                  "fixtures/updates/v1/ont2.ttl" => "v1/ont2.ttl",
                  "fixtures/updates/v1/ont3.ttl" => "v1/ont3.ttl",
                  "fixtures/updates/v1/ont4.ttl" => "v1/ont4.ttl" });
    let mut env = OntoEnv::init(cfg1, false)?;
    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 4);

    // copy files from dir/v2 to dir/v1
    setup!(&dir, {"fixtures/updates/v1/ont1.ttl" => "v1/ont1.ttl",
                  "fixtures/updates/v1/ont2.ttl" => "v1/ont2.ttl",
                  "fixtures/updates/v1/ont4.ttl" => "v1/ont4.ttl",
                  "fixtures/updates/v2/ont3.ttl" => "v1/ont3.ttl",
                  "fixtures/updates/v2/ont5.ttl" => "v1/ont5.ttl",
    });

    let updates = env.get_updated_locations()?;
    assert_eq!(updates.len(), 1);
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_dependency_closure() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    setup!(&dir, {"fixtures/brick-stuff/Brick-1.3.ttl" => "Brick-1.3.ttl",
                  "fixtures/brick-stuff/support/SCHEMA-FACADE_QUDT-v2.1.ttl" => "support/SCHEMA-FACADE_QUDT-v2.1.ttl",
                  "fixtures/brick-stuff/support/SCHEMA_QUDT_NoOWL-v2.1.ttl" => "support/SCHEMA_QUDT_NoOWL-v2.1.ttl",
                  "fixtures/brick-stuff/support/SHACL-SCHEMA-SUPPLEMENT_QUDT-v2.1.ttl" => "support/SHACL-SCHEMA-SUPPLEMENT_QUDT-v2.1.ttl",
                  "fixtures/brick-stuff/support/VOCAB_QUDT-DIMENSION-VECTORS-v2.1.ttl" => "support/VOCAB_QUDT-DIMENSION-VECTORS-v2.1.ttl",
                  "fixtures/brick-stuff/support/VOCAB_QUDT-PREFIX-v2.1.ttl" => "support/VOCAB_QUDT-PREFIX-v2.1.ttl",
                  "fixtures/brick-stuff/support/VOCAB_QUDT-QUANTITY-KINDS-ALL-v2.1.ttl" => "support/VOCAB_QUDT-QUANTITY-KINDS-ALL-v2.1.ttl",
                  "fixtures/brick-stuff/support/VOCAB_QUDT-SYSTEM-OF-UNITS-ALL-v2.1.ttl" => "support/VOCAB_QUDT-SYSTEM-OF-UNITS-ALL-v2.1.ttl",
                  "fixtures/brick-stuff/support/VOCAB_QUDT-UNITS-ALL-v2.1.ttl" => "support/VOCAB_QUDT-UNITS-ALL-v2.1.ttl",
                  "fixtures/brick-stuff/support/VOCAB_QUDT-UNITS-CURRENCY-v2.1.ttl" => "support/VOCAB_QUDT-UNITS-CURRENCY-v2.1.ttl",
                  "fixtures/brick-stuff/support/bacnet.ttl" => "support/bacnet.ttl",
                  "fixtures/brick-stuff/support/brickpatches.ttl" => "support/brickpatches.ttl",
                  "fixtures/brick-stuff/support/rec.ttl" => "support/rec.ttl",
                  "fixtures/brick-stuff/support/shacl.ttl" => "support/shacl.ttl",
                  "fixtures/brick-stuff/support/dash.ttl" => "support/dash.ttl",
                  "fixtures/brick-stuff/support/vaem.xml" => "support/vaem.xml",
                  "fixtures/brick-stuff/support/dtype.xml" => "support/dtype.xml",
                  "fixtures/brick-stuff/support/skos.ttl" => "support/skos.ttl",
                  "fixtures/brick-stuff/support/recimports.ttl" => "support/recimports.ttl",
                  "fixtures/brick-stuff/support/ref-schema.ttl" => "support/ref-schema.ttl"});
    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update()?;

    assert_eq!(env.stats()?.num_graphs, 20);

    let ont1 = NamedNodeRef::new("https://brickschema.org/schema/1.3/Brick")?;
    let ont_graph = env.resolve(ResolveTarget::Graph(ont1.into())).unwrap();
    let closure = env.get_dependency_closure(&ont_graph).unwrap();
    assert_eq!(closure.len(), 19);
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_dag_structure() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    setup!(&dir, {"fixtures/rdftest/ontology1.ttl" => "ontology1.ttl",
                  "fixtures/rdftest/ontology2.ttl" => "ontology2.ttl",
                  "fixtures/rdftest/ontology3.ttl" => "ontology3.ttl",
                  "fixtures/rdftest/ontology4.ttl" => "ontology4.ttl",
                  "fixtures/rdftest/ontology5.ttl" => "ontology5.ttl",
                  "fixtures/rdftest/ontology6.ttl" => "ontology6.ttl"});

    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update()?;

    // should have 6 ontologies in the environment
    assert_eq!(env.stats()?.num_graphs, 6);

    // ont2 => {ont2, ont1}

    // get the graph for ontology2
    let ont2 = NamedNodeRef::new("http://example.org/ontology2")?;
    let ont_graph = env.resolve(ResolveTarget::Graph(ont2.into())).unwrap();
    let closure = env.get_dependency_closure(&ont_graph).unwrap();
    assert_eq!(closure.len(), 2);
    let union = env.get_union_graph(&closure, None, None)?;
    assert_eq!(union.len(), 4);
    let union = env.get_union_graph(&closure, None, Some(false))?;
    assert_eq!(union.len(), 5);

    // ont3 => {ont3, ont2, ont1}
    let ont3 = NamedNodeRef::new("http://example.org/ontology3")?;
    let ont_graph = env.resolve(ResolveTarget::Graph(ont3.into())).unwrap();
    let closure = env.get_dependency_closure(&ont_graph).unwrap();
    assert_eq!(closure.len(), 3);
    let union = env.get_union_graph(&closure, None, None)?;
    assert_eq!(union.len(), 5);
    let union = env.get_union_graph(&closure, None, Some(false))?;
    assert_eq!(union.len(), 8);

    // ont5 => {ont5, ont4, ont3, ont2, ont1}
    let ont5 = NamedNodeRef::new("http://example.org/ontology5")?;
    let ont_graph = env.resolve(ResolveTarget::Graph(ont5.into())).unwrap();
    let closure = env.get_dependency_closure(&ont_graph).unwrap();
    assert_eq!(closure.len(), 5);
    let union = env.get_union_graph(&closure, None, None)?;
    assert_eq!(union.len(), 7);
    let union = env.get_union_graph(&closure, None, Some(false))?;
    // print the union
    assert_eq!(union.len(), 14);

    Ok(())
}

// === Initialization Tests Translated from Python ===

#[test]
fn test_init_with_config_new_dir() -> Result<()> {
    let dir = TempDir::new("ontoenv_init_new")?;
    let env_path = dir.path().join("new_env");
    // Ensure the directory does not exist initially
    assert!(!env_path.exists());

    let cfg = Config::new(
        env_path.clone(),             // root path
        Some(vec![env_path.clone()]), // search paths
        &["*.ttl"],
        &[""],
        false, // require_ontology_names
        false, // strict
        false, // offline
        "default".to_string(),
        false, // search_imports (assuming false if not specified)
        false, // temporary
    )?;

    // Initialize with recreate=true (implicit in init)
    let env = OntoEnv::init(cfg, true)?; // recreate = true

    let ontoenv_meta_dir = env_path.join(".ontoenv");
    assert!(ontoenv_meta_dir.is_dir());
    assert!(env.store_path().is_some()); // Should have a store path for non-temporary
    assert!(env.store_path().unwrap().starts_with(&ontoenv_meta_dir));

    teardown(dir);
    Ok(())
}

#[test]
fn test_init_with_config_existing_empty_dir() -> Result<()> {
    let dir = TempDir::new("ontoenv_init_empty")?;
    let env_path = dir.path().join("empty_env");
    std::fs::create_dir(&env_path)?;
    assert!(env_path.is_dir());
    assert!(std::fs::read_dir(&env_path)?.next().is_none()); // Check empty

    let cfg = Config::new(
        env_path.clone(),
        Some(vec![env_path.clone()]),
        &["*.ttl"],
        &[""],
        false,
        false,
        false,
        "default".to_string(),
        false,
        false,
    )?;

    // Initialize with recreate=true
    let env = OntoEnv::init(cfg, true)?;

    let ontoenv_meta_dir = env_path.join(".ontoenv");
    assert!(ontoenv_meta_dir.is_dir());
    assert!(env.store_path().is_some());
    assert!(env.store_path().unwrap().starts_with(&ontoenv_meta_dir));

    teardown(dir);
    Ok(())
}

#[test]
fn test_init_load_from_existing_dir() -> Result<()> {
    let dir = TempDir::new("ontoenv_load_existing")?;
    let env_path = dir.path().join("existing_env");
    std::fs::create_dir(&env_path)?;

    // Create a dummy environment first
    let cfg = Config::new(
        env_path.clone(),
        Some(vec![env_path.clone()]),
        &["*.ttl"],
        &[""],
        false,
        false,
        false,
        "default".to_string(),
        false,
        false,
    )?;
    let mut initial_env = OntoEnv::init(cfg, true)?;
    initial_env.flush()?; // Ensure store is created/flushed
    let expected_store_path = initial_env.store_path().unwrap().to_path_buf();
    initial_env.save_to_directory()?; // Save config and env state
    drop(initial_env); // Drop to release file locks if any

    // Now load from the existing directory
    let loaded_env = OntoEnv::load_from_directory(env_path.clone(), false)?; // read_only = false

    assert!(env_path.join(".ontoenv").is_dir());
    assert_eq!(loaded_env.store_path(), Some(expected_store_path.as_path()));

    teardown(dir);
    Ok(())
}

#[test]
fn test_init_recreate_existing_dir() -> Result<()> {
    let dir = TempDir::new("ontoenv_recreate")?;
    let env_path = dir.path().join("recreate_env");
    std::fs::create_dir(&env_path)?;

    // Create a dummy environment first
    let cfg = Config::new(
        env_path.clone(),
        Some(vec![env_path.clone()]),
        &["*.ttl"],
        &[""],
        false,
        false,
        false,
        "default".to_string(),
        false,
        false,
    )?;
    let mut initial_env = OntoEnv::init(cfg.clone(), true)?;
    // Add a dummy file to check for removal
    let dummy_file_path = env_path.join(".ontoenv").join("dummy.txt");
    std::fs::File::create(&dummy_file_path)?;
    assert!(dummy_file_path.exists());
    initial_env.flush()?;
    initial_env.save_to_directory()?;
    drop(initial_env);

    // Recreate the environment
    let recreated_env = OntoEnv::init(cfg, true)?; // recreate = true

    assert!(env_path.join(".ontoenv").is_dir());
    // Check if the dummy file is gone
    assert!(!dummy_file_path.exists());
    // Check if the environment is empty (e.g., no ontologies)
    assert_eq!(recreated_env.ontologies().len(), 0);
    assert_eq!(recreated_env.stats()?.num_ontologies, 0);

    teardown(dir);
    Ok(())
}

#[test]
fn test_init_read_only() -> Result<()> {
    let dir = TempDir::new("ontoenv_readonly")?;
    let env_path = dir.path().join("readonly_env");
    std::fs::create_dir(&env_path)?;

    // Create a dummy environment first
    let cfg = Config::new(
        env_path.clone(),
        Some(vec![env_path.clone()]),
        &["*.ttl"],
        &[""],
        false,
        false,
        false,
        "default".to_string(),
        false,
        false,
    )?;
    let mut initial_env = OntoEnv::init(cfg, true)?;
    initial_env.flush()?;
    initial_env.save_to_directory()?;
    drop(initial_env);

    // Load in read-only mode
    let mut loaded_env = OntoEnv::load_from_directory(env_path.clone(), true)?; // read_only = true

    // Attempting to modify should fail.
    // We need a file that *could* be added if not read-only.
    let dummy_ont_path = dir.path().join("dummy.ttl");
    std::fs::write(
        &dummy_ont_path,
        "<urn:dummy> a <http://www.w3.org/2002/07/owl#Ontology> .",
    )?;
    let location = OntologyLocation::File(dummy_ont_path);

    // The OntoEnv::add method requires &mut self.
    // The underlying ReadOnlyPersistentGraphIO::add should return an error.
    let add_result = loaded_env.add(location, false);

    assert!(add_result.is_err());
    // Check if the error message indicates read-only restriction
    // Note: The exact error might depend on the GraphIO implementation details.
    // Assuming ReadOnlyPersistentGraphIO::add returns a specific error.
    // If GraphIO trait doesn't have 'add', this test might need adjustment based on how OntoEnv handles it.
    // Let's assume GraphIO has 'add' and ReadOnly returns an error like below.
    assert!(add_result
        .unwrap_err()
        .to_string()
        .contains("Cannot add to read-only store"));

    teardown(dir);
    Ok(())
}

#[test]
fn test_init_path_no_env_error() -> Result<()> {
    let dir = TempDir::new("ontoenv_path_no_env")?;
    let env_path = dir.path().join("no_env_here");
    std::fs::create_dir(&env_path)?; // Create the directory, but not .ontoenv inside it
    assert!(env_path.is_dir());
    assert!(!env_path.join(".ontoenv").exists());

    // Attempt to load from the directory without .ontoenv
    let load_result = OntoEnv::load_from_directory(env_path.clone(), false);

    assert!(load_result.is_err());
    let err_msg = load_result.unwrap_err().to_string();
    // Check for the specific error message from load_from_directory
    let expected_meta_path = env_path.join(".ontoenv");
    assert!(err_msg.contains(&format!(
        "OntoEnv directory not found at: {:?}",
        expected_meta_path
    )));

    teardown(dir);
    Ok(())
}

#[test]
fn test_init_temporary() -> Result<()> {
    let dir = TempDir::new("ontoenv_temporary")?;
    let env_path = dir.path().join("temp_env_root");
    // Temporary envs shouldn't persist to disk relative to root

    let cfg = Config::new(
        env_path.clone(),             // Root path (shouldn't be used for storage)
        Some(vec![env_path.clone()]), // Search path (can still be used)
        &["*.ttl"],
        &[""],
        false, // require_ontology_names
        false, // strict
        false, // offline
        "default".to_string(),
        false, // search_imports
        true,  // temporary = true
    )?;

    let mut env = OntoEnv::init(cfg, false)?; // recreate doesn't matter much for temp

    // .ontoenv directory should NOT be created at the root
    assert!(!env_path.join(".ontoenv").exists());

    // store_path() should return None for temporary envs
    assert!(env.store_path().is_none());

    // Check if adding works in memory (should not raise read-only error)
    // Create a dummy ontology file to add
    let dummy_ont_path = dir.path().join("dummy_temp.ttl");
    std::fs::write(
        &dummy_ont_path,
        "<urn:dummy_temp> a <http://www.w3.org/2002/07/owl#Ontology> .",
    )?;
    let location = OntologyLocation::File(dummy_ont_path);

    let add_result = env.add(location, false);
    assert!(add_result.is_ok()); // Should succeed in memory

    // Verify the ontology was added (in memory)
    assert_eq!(env.ontologies().len(), 1);
    assert!(env
        .resolve(ResolveTarget::Graph(
            NamedNodeRef::new("urn:dummy_temp")?.into()
        ))
        .is_some());

    teardown(dir);
    Ok(())
}
