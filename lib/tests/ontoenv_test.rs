use anyhow::Result;
use ontoenv::config::{Config, HowCreated};
use ontoenv::ontology::OntologyLocation;
use ontoenv::OntoEnv;
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
    let mut env = OntoEnv::new(cfg, false)?;
    env.update()?;
    assert_eq!(env.num_graphs(), 4);
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
    )?;
    let mut env = OntoEnv::new(cfg, false)?;
    env.update()?;
    assert_eq!(env.num_graphs(), 4);
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
    )?;
    let mut env = OntoEnv::new(cfg1, false)?;
    env.update()?;
    assert_eq!(env.num_graphs(), 1);
    assert_eq!(env.num_triples()?, 5);
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
    let mut env = OntoEnv::new(cfg, false)?;
    env.update()?;
    let old_num_triples = env.num_triples()?;
    assert_eq!(env.num_graphs(), 4);

    // updating again shouldn't add anything
    env.update()?;
    assert_eq!(env.num_graphs(), 4);
    assert_eq!(env.num_triples()?, old_num_triples);

    // remove ont2.ttl
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl"});

    env.update()?;
    assert_eq!(env.num_graphs(), 3);

    // copy ont4.ttl back
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont2.ttl" => "ont2.ttl",
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl" });
    env.update()?;
    assert_eq!(env.num_graphs(), 4);

    teardown(dir);
    Ok(())
}

#[test]
fn test_recreate() -> Result<()> {
    let dir = TempDir::new("ontoenv")?;
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont2.ttl" => "ont2.ttl",
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl" });
    let cfg = default_config(&dir);
    let env = OntoEnv::new(cfg, false)?;
    env.save_to_directory()?;
    assert_eq!(env.get_how_created(), HowCreated::New);
    // create a new env with the same config. This should still work.
    let cfg = default_config(&dir);
    let env = OntoEnv::new(cfg, false)?;
    env.save_to_directory()?;
    assert_eq!(env.get_how_created(), HowCreated::SameConfig);
    // change the config; this should trigger a recreation of the environment
    let cfg = default_config_ttl_only(&dir);
    let env = OntoEnv::new(cfg, false)?;
    env.save_to_directory()?;
    assert_eq!(env.get_how_created(), HowCreated::RecreatedDifferentConfig);
    // now try to recreate the env with the same config but with recreate set to true
    let cfg = default_config(&dir);
    let env = OntoEnv::new(cfg, true)?;
    env.save_to_directory()?;
    assert_eq!(env.get_how_created(), HowCreated::RecreatedFlag);

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
    let mut env = OntoEnv::new(cfg, false)?;
    env.update()?;

    let ont1 = NamedNodeRef::new("urn:ont1")?;
    let ont = env
        .get_ontology_by_name(ont1)
        .ok_or(anyhow::anyhow!("Ontology not found"))?;
    assert_eq!(ont.imports.len(), 1);
    assert!(ont.location().unwrap().is_file());

    let ont2 = NamedNodeRef::new("urn:ont2")?;
    let ont = env
        .get_ontology_by_name(ont2)
        .ok_or(anyhow::anyhow!("Ontology not found"))?;
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
    let mut env = OntoEnv::new(cfg, false)?;
    env.update()?;

    let ont1_path = dir.path().join("ont1.ttl");
    let loc = OntologyLocation::from_str(
        ont1_path
            .to_str()
            .ok_or(anyhow::anyhow!("Failed to convert to string"))?,
    )?;
    let ont = env
        .get_ontology_by_location(&loc)
        .ok_or(anyhow::anyhow!("Ontology not found"))?;
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
    let mut env = OntoEnv::new(cfg, false)?;
    env.update()?;
    assert_eq!(env.num_graphs(), 4);
    env.save_to_directory()?;
    // drop env
    env.close();

    // reload env
    let cfg_location = dir.path().join(".ontoenv").join("ontoenv.json");
    let env2 = OntoEnv::from_file(cfg_location.as_path(), true)?;
    assert_eq!(env2.num_graphs(), 4);
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
    let mut env = OntoEnv::new(cfg1, false)?;
    env.update()?;
    assert_eq!(env.num_graphs(), 4);

    let ont_path = dir.path().join("v2/ont5.ttl");
    let loc = OntologyLocation::from_str(
        ont_path
            .to_str()
            .ok_or(anyhow::anyhow!("Failed to convert to string"))?,
    )?;
    env.add(loc)?;
    assert_eq!(env.num_graphs(), 5);
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
    let mut env = OntoEnv::new(cfg1, false)?;
    env.update()?;
    assert_eq!(env.num_graphs(), 4);

    // copy files from dir/v2 to dir/v1
    setup!(&dir, {"fixtures/updates/v1/ont1.ttl" => "v1/ont1.ttl",
                  "fixtures/updates/v1/ont2.ttl" => "v1/ont2.ttl",
                  "fixtures/updates/v1/ont4.ttl" => "v1/ont4.ttl",
                  "fixtures/updates/v2/ont3.ttl" => "v1/ont3.ttl",
                  "fixtures/updates/v2/ont5.ttl" => "v1/ont5.ttl",
    });
    env.update()?;

    assert_eq!(env.num_graphs(), 5);
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
    let mut env = OntoEnv::new(cfg1, false)?;
    env.update()?;
    assert_eq!(env.num_graphs(), 4);

    // copy files from dir/v2 to dir/v1
    setup!(&dir, {"fixtures/updates/v1/ont1.ttl" => "v1/ont1.ttl",
                  "fixtures/updates/v1/ont2.ttl" => "v1/ont2.ttl",
                  "fixtures/updates/v1/ont4.ttl" => "v1/ont4.ttl",
                  "fixtures/updates/v2/ont3.ttl" => "v1/ont3.ttl",
                  "fixtures/updates/v2/ont5.ttl" => "v1/ont5.ttl",
    });

    let updates = env.get_updated_files()?;
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
                  "fixtures/brick-stuff/support/VOCAB_QUDT-PREFIXES-v2.1.ttl" => "support/VOCAB_QUDT-PREFIXES-v2.1.ttl",
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
    let mut env = OntoEnv::new(cfg, false)?;
    env.update()?;

    assert_eq!(env.num_graphs(), 21);

    let ont1 = NamedNodeRef::new("https://brickschema.org/schema/1.3/Brick")?;
    let ont_graph = env.get_ontology_by_name(ont1).unwrap();
    let closure = env.get_dependency_closure(ont_graph.id()).unwrap();
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
    let mut env = OntoEnv::new(cfg, false)?;
    env.update()?;

    // should have 6 ontologies in the environment
    assert_eq!(env.num_graphs(), 6);

    // ont2 => {ont2, ont1}

    // get the graph for ontology2
    let ont2 = NamedNodeRef::new("http://example.org/ontology2")?;
    let ont_graph = env.get_ontology_by_name(ont2).unwrap();
    let closure = env.get_dependency_closure(ont_graph.id()).unwrap();
    assert_eq!(closure.len(), 2);
    let union = env.get_union_graph(&closure, None, None)?;
    assert_eq!(union.len(), 4);
    let union = env.get_union_graph(&closure, None, Some(false))?;
    assert_eq!(union.len(), 5);

    // ont3 => {ont3, ont2, ont1}
    let ont3 = NamedNodeRef::new("http://example.org/ontology3")?;
    let ont_graph = env.get_ontology_by_name(ont3).unwrap();
    let closure = env.get_dependency_closure(ont_graph.id()).unwrap();
    assert_eq!(closure.len(), 3);
    let union = env.get_union_graph(&closure, None, None)?;
    assert_eq!(union.len(), 5);
    let union = env.get_union_graph(&closure, None, Some(false))?;
    assert_eq!(union.len(), 8);

    // ont5 => {ont5, ont4, ont3, ont2, ont1}
    let ont5 = NamedNodeRef::new("http://example.org/ontology5")?;
    let ont_graph = env.get_ontology_by_name(ont5).unwrap();
    let closure = env.get_dependency_closure(ont_graph.id()).unwrap();
    assert_eq!(closure.len(), 5);
    let union = env.get_union_graph(&closure, None, None)?;
    assert_eq!(union.len(), 7);
    let union = env.get_union_graph(&closure, None, Some(false))?;
    // print the union
    assert_eq!(union.len(), 14);

    Ok(())
}
