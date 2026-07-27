use anyhow::Result;
use ontoenv::api::{OntoEnv, ResolveTarget};
use ontoenv::config::{Config, ConfigOverrides};
use ontoenv::consts::IMPORTS;
use ontoenv::ontology::OntologyLocation;
use ontoenv::options::{CacheMode, Overwrite, RefreshStrategy};
use ontoenv::ToUriString;
use oxigraph::io::RdfFormat;
use oxigraph::model::NamedNode;
use oxigraph::model::NamedNodeRef;
use oxigraph::model::NamedOrBlankNodeRef;
use oxigraph::model::TermRef;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tempfile::{Builder, TempDir};

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
            // Ensure the parent directories exist
            if let Some(parent) = dest_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent).expect("Failed to create parent directories");
                }
            }

            copy_file(&source_path, &dest_path).expect(format!("Failed to copy file from {} to {}", source_path.display(), dest_path.display()).as_str());

            // modify the 'last modified' time to the current time.
            // We must open with .write(true) to get permissions
            // to set metadata on Windows.
            let current_time = std::time::SystemTime::now();
            let dest_file = std::fs::OpenOptions::new()
                .write(true) // Request write access
                .open(&dest_path)
                .expect(format!("Failed to open file {} with write perms", dest_path.display()).as_str());

            dest_file.set_modified(current_time)
                .expect(format!("Failed to set modified time for file {}", dest_path.display()).as_str());
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

fn new_tempdir(prefix: &str) -> Result<TempDir> {
    Ok(Builder::new().prefix(prefix).tempdir()?)
}

fn cached_env(dir: &TempDir) -> Result<OntoEnv> {
    let config = Config::builder()
        .root(dir.path().into())
        .locations(vec![dir.path().into()])
        .includes(&["*.ttl"])
        .excludes(&[] as &[&str])
        .require_ontology_names(false)
        .strict(false)
        .offline(true)
        .temporary(true)
        .use_cached_ontologies(CacheMode::Enabled)
        .build()?;
    OntoEnv::init(config, true)
}

fn default_config(dir: &TempDir) -> Config {
    Config::builder()
        .root(dir.path().into())
        .locations(vec![dir.path().into()])
        .includes(&["*.ttl", "*.xml"])
        .excludes(&[] as &[&str])
        .strict(false)
        .offline(true)
        .build()
        .unwrap()
}

fn default_config_with_subdir(dir: &TempDir, path: &str) -> Config {
    Config::builder()
        .root(dir.path().into())
        .locations(vec![dir.path().join(path)])
        .includes(&["*.ttl"])
        .excludes(&[] as &[&str])
        .offline(true)
        .build()
        .unwrap()
}

#[test]
fn init_respects_cache_mode_for_implicit_updates() -> Result<()> {
    let dir = new_tempdir("ontoenv-cache-mode")?;
    let a_path = dir.path().join("A.ttl");
    std::fs::write(
        &a_path,
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.com/A> a owl:Ontology .",
    )?;

    let cached_cfg = Config::builder()
        .root(dir.path().into())
        .locations(vec![dir.path().into()])
        .includes(&["*.ttl"])
        .offline(true)
        .temporary(true)
        .use_cached_ontologies(CacheMode::Enabled)
        .build()?;
    let env_cached = OntoEnv::init(cached_cfg, true)?;
    assert!(
        env_cached.ontologies().is_empty(),
        "cache-enabled mode should skip implicit discovery"
    );

    let eager_cfg = Config::builder()
        .root(dir.path().into())
        .locations(vec![dir.path().into()])
        .includes(&["*.ttl"])
        .offline(true)
        .temporary(true)
        .use_cached_ontologies(CacheMode::Disabled)
        .build()?;
    let env_eager = OntoEnv::init(eager_cfg, true)?;
    assert_eq!(
        env_eager.ontologies().len(),
        1,
        "cache-disabled mode should eagerly load ontologies"
    );

    teardown(dir);
    Ok(())
}

fn teardown(dir: TempDir) {
    let _ = dir.close();
}

#[test]
fn ontology_regex_filters_exclude() -> Result<()> {
    let dir = new_tempdir("ontoenv-regex-filter")?;
    let a_path = dir.path().join("A.ttl");
    let b_path = dir.path().join("B.ttl");

    let a_iri = "http://example.com/A";
    let b_iri = "http://example.com/B";
    std::fs::write(
        &a_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{iri}> a owl:Ontology .\n",
            iri = a_iri
        ),
    )?;
    std::fs::write(
        &b_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{iri}> a owl:Ontology .\n",
            iri = b_iri
        ),
    )?;

    let config = Config::builder()
        .root(dir.path().into())
        .locations(vec![dir.path().into()])
        .includes(&["*.ttl"])
        .exclude_ontologies(&["example.com/B"])
        .offline(true)
        .build()?;

    let env = OntoEnv::init(config, true)?;
    let names: Vec<String> = env
        .ontologies()
        .keys()
        .map(|id| id.to_uri_string())
        .collect();

    assert!(names.iter().any(|n| n.contains("example.com/A")));
    assert!(!names.iter().any(|n| n.contains("example.com/B")));

    teardown(dir);
    Ok(())
}

#[test]
fn import_graph_merges_closure_and_removes_imports() -> Result<()> {
    use ontoenv::consts::{IMPORTS, ONTOLOGY, PREFIXES, TYPE};
    use oxigraph::model::Triple;
    let dir = new_tempdir("ontoenv-import-merge")?;

    // A imports B, B imports A (cycle)
    let a_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex.org/> .
<http://ex.org/A> a owl:Ontology ;
  owl:imports <http://ex.org/B> .
ex:shape sh:prefixes <http://ex.org/A> .
ex:a ex:p ex:o .
"#;
    let b_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix ex: <http://ex.org/> .
<http://ex.org/B> a owl:Ontology ;
  owl:imports <http://ex.org/A> .
ex:b ex:p ex:o .
"#;
    fs::write(dir.path().join("A.ttl"), a_ttl)?;
    fs::write(dir.path().join("B.ttl"), b_ttl)?;

    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update_all(false)?;

    let a_name = NamedNodeRef::new_unchecked("http://ex.org/A");
    let a_id = env
        .resolve(ResolveTarget::Graph(a_name.into()))
        .expect("A should resolve");

    let merged = env.import_graph(&a_id, -1)?;

    // Should contain data from both ontologies
    let a_triple = Triple::new(
        NamedNodeRef::new_unchecked("http://ex.org/a"),
        NamedNodeRef::new_unchecked("http://ex.org/p"),
        NamedNodeRef::new_unchecked("http://ex.org/o"),
    );
    let b_triple = Triple::new(
        NamedNodeRef::new_unchecked("http://ex.org/b"),
        NamedNodeRef::new_unchecked("http://ex.org/p"),
        NamedNodeRef::new_unchecked("http://ex.org/o"),
    );
    assert!(
        merged.contains(a_triple.as_ref()),
        "Merged graph missing A data"
    );
    assert!(
        merged.contains(b_triple.as_ref()),
        "Merged graph missing B data"
    );

    // sh:prefixes should be rewritten onto the root (base) ontology
    let prefixes: Vec<_> = merged.triples_for_predicate(PREFIXES).collect();
    assert!(
        !prefixes.is_empty(),
        "Merged graph should contain rewritten sh:prefixes"
    );
    assert!(
        prefixes
            .iter()
            .all(|t| t.object == TermRef::NamedNode(a_id.name())),
        "All sh:prefixes objects should be the root ontology"
    );

    // owl:imports should be rewritten onto the root (base) ontology
    let imports: Vec<_> = merged
        .triples_for_predicate(IMPORTS)
        .filter(|t| t.subject == NamedOrBlankNodeRef::NamedNode(a_id.name()))
        .collect();
    assert!(
        !imports.is_empty(),
        "Merged graph should contain rewritten imports on the root"
    );
    assert!(
        imports
            .iter()
            .all(|t| t.subject == NamedOrBlankNodeRef::NamedNode(a_id.name())),
        "All imports should be on the root ontology"
    );

    // Only one owl:Ontology declaration (root) should remain
    let ontology_decls = merged
        .triples_for_object(ONTOLOGY)
        .filter(|t| t.predicate == TYPE)
        .count();
    assert_eq!(
        ontology_decls, 1,
        "Should retain only the root ontology declaration"
    );

    teardown(dir);
    Ok(())
}

#[test]
fn import_graph_handles_cycles() -> Result<()> {
    use ontoenv::consts::{IMPORTS, ONTOLOGY, TYPE};

    let dir = new_tempdir("ontoenv-import-cycle")?;

    let a_path = dir.path().join("A.ttl");
    let b_path = dir.path().join("B.ttl");
    let a_iri = url::Url::from_file_path(&a_path).unwrap().to_string();
    let b_iri = url::Url::from_file_path(&b_path).unwrap().to_string();

    fs::write(
        &a_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix ex: <http://example.com/A#> .\n<{a}> a owl:Ontology ; owl:imports <{b}> .\nex:A a owl:Class .\n",
            a = a_iri,
            b = b_iri
        ),
    )?;
    fs::write(
        &b_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix ex: <http://example.com/B#> .\n<{b}> a owl:Ontology ; owl:imports <{a}> .\nex:B a owl:Class .\n",
            a = a_iri,
            b = b_iri
        ),
    )?;

    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.add(
        OntologyLocation::File(a_path.clone()),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;
    env.add(
        OntologyLocation::File(b_path.clone()),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;

    let a_id = env
        .resolve(ResolveTarget::Location(OntologyLocation::File(a_path)))
        .unwrap();
    let merged = env.import_graph(&a_id, -1)?;

    // Single root ontology
    let ontology_decls = merged
        .triples_for_object(ONTOLOGY)
        .filter(|t| t.predicate == TYPE)
        .count();
    assert_eq!(ontology_decls, 1);

    // Imports rewritten onto root with no self-loop
    let imports: Vec<_> = merged
        .triples_for_predicate(IMPORTS)
        .filter(|t| t.subject == NamedOrBlankNodeRef::NamedNode(a_id.name()))
        .collect();
    assert_eq!(imports.len(), 1);
    if let TermRef::NamedNode(obj) = imports[0].object {
        assert_eq!(obj.as_str(), b_iri);
    } else {
        panic!("Import object was not a NamedNode");
    }

    // No imports hanging off B
    assert_eq!(
        merged
            .triples_for_predicate(IMPORTS)
            .filter(|t| {
                t.subject == NamedOrBlankNodeRef::NamedNode(NamedNodeRef::new_unchecked(&b_iri))
            })
            .count(),
        0
    );

    // Data from both ontologies present
    assert!(merged
        .iter()
        .any(|t| format!("{:?}", t.subject).contains("#A")));
    assert!(merged
        .iter()
        .any(|t| format!("{:?}", t.subject).contains("#B")));

    teardown(dir);
    Ok(())
}

#[test]
fn union_graph_orders_root_for_sh_prefixes() -> Result<()> {
    use ontoenv::consts::PREFIXES;
    use oxigraph::model::TermRef;

    let dir = new_tempdir("ontoenv-prefix-root-order")?;

    let a_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex.org/> .
<http://ex.org/A> a owl:Ontology ;
  owl:imports <http://ex.org/B> .
ex:shape sh:prefixes <http://ex.org/A> .
"#;
    let b_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix exb: <http://ex.org/b#> .
<http://ex.org/B> a owl:Ontology .
exb:shape sh:prefixes <http://ex.org/B> .
"#;
    fs::write(dir.path().join("A.ttl"), a_ttl)?;
    fs::write(dir.path().join("B.ttl"), b_ttl)?;

    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update_all(false)?;

    let a_name = NamedNodeRef::new_unchecked("http://ex.org/A");
    let b_name = NamedNodeRef::new_unchecked("http://ex.org/B");
    let a_id = env
        .resolve(ResolveTarget::Graph(a_name.into()))
        .expect("A should resolve");
    let b_id = env
        .resolve(ResolveTarget::Graph(b_name.into()))
        .expect("B should resolve");

    // Pass explicit root (A is the root ontology).
    let union = env.get_union_graph(vec![&b_id, &a_id], a_id.name(), Some(true), Some(true))?;

    let prefixes: Vec<_> = union.dataset.quads_for_predicate(PREFIXES).collect();
    assert!(!prefixes.is_empty(), "Expected sh:prefixes quads");
    assert!(
        prefixes
            .iter()
            .all(|q| q.object == TermRef::NamedNode(a_id.name())),
        "All sh:prefixes objects should point to the root ontology"
    );

    teardown(dir);
    Ok(())
}

#[test]
fn union_graph_rewrites_sh_prefixes_from_deep_dependency() -> Result<()> {
    use ontoenv::consts::PREFIXES;
    use oxigraph::model::TermRef;

    let dir = new_tempdir("ontoenv-prefix-root-deep")?;

    let a_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://ex.org/> .
<http://ex.org/A> a owl:Ontology ;
  owl:imports <http://ex.org/B> .
ex:shape sh:prefixes <http://ex.org/A> .
"#;
    let b_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix exb: <http://ex.org/b#> .
<http://ex.org/B> a owl:Ontology ;
  owl:imports <http://ex.org/C> .
exb:shape sh:prefixes <http://ex.org/B> .
"#;
    let c_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix exc: <http://ex.org/c#> .
<http://ex.org/C> a owl:Ontology .
exc:shape sh:prefixes <http://ex.org/C> .
"#;
    fs::write(dir.path().join("A.ttl"), a_ttl)?;
    fs::write(dir.path().join("B.ttl"), b_ttl)?;
    fs::write(dir.path().join("C.ttl"), c_ttl)?;

    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update_all(false)?;

    let a_name = NamedNodeRef::new_unchecked("http://ex.org/A");
    let b_name = NamedNodeRef::new_unchecked("http://ex.org/B");
    let c_name = NamedNodeRef::new_unchecked("http://ex.org/C");
    let a_id = env
        .resolve(ResolveTarget::Graph(a_name.into()))
        .expect("A should resolve");
    let b_id = env
        .resolve(ResolveTarget::Graph(b_name.into()))
        .expect("B should resolve");
    let c_id = env
        .resolve(ResolveTarget::Graph(c_name.into()))
        .expect("C should resolve");

    // Pass explicit root (A is the root ontology).
    let union = env.get_union_graph(
        vec![&b_id, &c_id, &a_id],
        a_id.name(),
        Some(true),
        Some(true),
    )?;

    let prefixes: Vec<_> = union.dataset.quads_for_predicate(PREFIXES).collect();
    assert!(!prefixes.is_empty(), "Expected sh:prefixes quads");
    assert!(
        prefixes
            .iter()
            .all(|q| q.object == TermRef::NamedNode(a_id.name())),
        "All sh:prefixes objects should point to the root ontology"
    );

    teardown(dir);
    Ok(())
}

#[test]
fn explicit_union_includes_closures_only_when_requested() -> Result<()> {
    let dir = new_tempdir("ontoenv-explicit-union")?;

    let a_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
<http://ex.org/A> a owl:Ontology ;
  owl:imports <http://ex.org/B> .
<http://ex.org/A#Class> a <http://ex.org/Marker> .
"#;
    let b_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://ex.org/B> a owl:Ontology .
<http://ex.org/B#Class> a <http://ex.org/Marker> .
"#;
    let c_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://ex.org/C> a owl:Ontology .
<http://ex.org/C#Class> a <http://ex.org/Marker> .
"#;
    fs::write(dir.path().join("A.ttl"), a_ttl)?;
    fs::write(dir.path().join("B.ttl"), b_ttl)?;
    fs::write(dir.path().join("C.ttl"), c_ttl)?;

    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update_all(false)?;

    let a = NamedNode::new("http://ex.org/A")?;
    let c = NamedNode::new("http://ex.org/C")?;
    let root = NamedNode::new("http://ex.org/UnionRoot")?;
    let b_class = NamedNodeRef::new_unchecked("http://ex.org/B#Class");
    let c_class = NamedNodeRef::new_unchecked("http://ex.org/C#Class");
    let rdf_type = NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    let owl_ontology = NamedNodeRef::new_unchecked("http://www.w3.org/2002/07/owl#Ontology");

    let explicit_only = env.get_explicit_union_graph(
        &[a.clone(), c.clone()],
        root.as_ref(),
        false,
        -1,
        Some(true),
        Some(true),
    )?;
    assert_eq!(explicit_only.graph_ids.len(), 2);
    assert!(
        explicit_only
            .dataset
            .iter()
            .all(|q| q.subject != NamedOrBlankNodeRef::NamedNode(b_class)),
        "B should not be included without closure expansion"
    );
    assert!(
        explicit_only
            .dataset
            .iter()
            .any(|q| q.subject == NamedOrBlankNodeRef::NamedNode(c_class)),
        "Explicitly listed C should be included"
    );

    let with_closures =
        env.get_explicit_union_graph(&[a, c], root.as_ref(), true, -1, Some(true), Some(true))?;
    assert_eq!(with_closures.graph_ids.len(), 3);
    assert!(
        with_closures
            .dataset
            .iter()
            .any(|q| q.subject == NamedOrBlankNodeRef::NamedNode(b_class)),
        "B should be included through A's closure"
    );
    let declarations: Vec<_> = with_closures
        .dataset
        .iter()
        .filter(|q| q.predicate == rdf_type && q.object == TermRef::NamedNode(owl_ontology))
        .collect();
    assert_eq!(declarations.len(), 1);
    assert_eq!(
        declarations[0].subject,
        NamedOrBlankNodeRef::NamedNode(root.as_ref())
    );

    teardown(dir);
    Ok(())
}

#[test]
fn union_graph_errors_on_conflicting_sh_prefix() -> Result<()> {
    let dir = new_tempdir("ontoenv-prefix-conflict")?;

    let a_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
<http://ex.org/A> a owl:Ontology ;
  owl:imports <http://ex.org/B> ;
  sh:declare [
    sh:prefix "ex" ;
    sh:namespace <http://example.com/ns/one#>
  ] .
"#;
    let b_ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
<http://ex.org/B> a owl:Ontology ;
  sh:declare [
    sh:prefix "ex" ;
    sh:namespace <http://example.com/ns/two#>
  ] .
"#;
    fs::write(dir.path().join("A.ttl"), a_ttl)?;
    fs::write(dir.path().join("B.ttl"), b_ttl)?;

    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update_all(false)?;

    let a_name = NamedNodeRef::new_unchecked("http://ex.org/A");
    let a_id = env
        .resolve(ResolveTarget::Graph(a_name.into()))
        .expect("A should resolve");

    let closure = env.get_closure(&a_id, -1)?;
    let result = env.get_union_graph(&closure, a_id.name(), Some(true), Some(true));
    let msg = match result {
        Err(e) => e.to_string(),
        Ok(_) => panic!("Expected a conflict error"),
    };
    assert!(
        msg.contains("Conflicting sh:prefix \"ex\""),
        "Error message should mention the conflicting prefix: {msg}"
    );

    teardown(dir);
    Ok(())
}

#[test]
fn import_graph_respects_recursion_depth() -> Result<()> {
    let dir = new_tempdir("ontoenv-import-depth")?;

    let a_path = dir.path().join("A.ttl");
    let b_path = dir.path().join("B.ttl");
    let c_path = dir.path().join("C.ttl");

    let a_iri = url::Url::from_file_path(&a_path).unwrap().to_string();
    let b_iri = url::Url::from_file_path(&b_path).unwrap().to_string();
    let c_iri = url::Url::from_file_path(&c_path).unwrap().to_string();

    fs::write(
        &a_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{a}> a owl:Ontology ; owl:imports <{b}> .",
            a = a_iri, b = b_iri
        ),
    )?;
    fs::write(
        &b_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{b}> a owl:Ontology ; owl:imports <{c}> .",
            b = b_iri, c = c_iri
        ),
    )?;
    fs::write(
        &c_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{c}> a owl:Ontology .",
            c = c_iri
        ),
    )?;

    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.add(
        OntologyLocation::File(a_path.clone()),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;
    env.add(
        OntologyLocation::File(b_path.clone()),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;
    env.add(
        OntologyLocation::File(c_path.clone()),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;

    let a_id = env
        .resolve(ResolveTarget::Location(OntologyLocation::File(a_path)))
        .unwrap();

    // depth 0: only A (no imports attached)
    let g0 = env.import_graph(&a_id, 0)?;
    let imports0 = g0
        .triples_for_predicate(IMPORTS)
        .filter(|t| t.subject == NamedOrBlankNodeRef::NamedNode(a_id.name()))
        .count();
    assert_eq!(imports0, 0, "depth 0 should not carry imports on root");

    // depth 1: A imports B
    let g1 = env.import_graph(&a_id, 1)?;
    let imports_b: Vec<_> = g1
        .triples_for_predicate(IMPORTS)
        .filter(|t| t.subject == NamedOrBlankNodeRef::NamedNode(a_id.name()))
        .collect();
    assert_eq!(imports_b.len(), 1);

    // depth -1: full closure, includes C
    let gfull = env.import_graph(&a_id, -1)?;
    let imports_full: Vec<_> = gfull
        .triples_for_predicate(IMPORTS)
        .filter(|t| t.subject == NamedOrBlankNodeRef::NamedNode(a_id.name()))
        .collect();
    assert_eq!(imports_full.len(), 2);

    teardown(dir);
    Ok(())
}

#[cfg(unix)]
mod unix_permission_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_find_files_skips_permission_denied_when_not_strict() -> Result<()> {
        let dir = new_tempdir("ontoenv-permissions")?;
        setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl" });

        let restricted_dir = dir.path().join("restricted");
        fs::create_dir_all(&restricted_dir)?;
        fs::write(
            restricted_dir.join("hidden.ttl"),
            "@prefix : <#> . :s :p :o .",
        )?;

        struct PermissionGuard {
            path: PathBuf,
            original: fs::Permissions,
        }

        impl Drop for PermissionGuard {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.path, self.original.clone());
            }
        }

        let guard = PermissionGuard {
            path: restricted_dir.clone(),
            original: fs::metadata(&restricted_dir)?.permissions(),
        };

        let mut denied = guard.original.clone();
        denied.set_mode(0o000);
        fs::set_permissions(&restricted_dir, denied)?;

        let cfg = default_config(&dir);
        let env = OntoEnv::init(cfg, false)?;
        let files = env.find_files()?;
        // `find_files` canonicalizes discovered paths (resolving symlinks such
        // as macOS `/var` -> `/private/var`), so the expected entry must be
        // canonicalized the same way to compare equal.
        let expected = OntologyLocation::File(ontoenv::ontology::canonicalize_file_path(
            &dir.path().join("ont1.ttl"),
        ));
        assert!(
            files.contains(&expected),
            "find_files should still collect readable entries"
        );

        drop(guard);
        teardown(dir);
        Ok(())
    }
}

#[cfg(windows)]
mod windows_permission_tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn test_find_files_skips_sharing_violation_when_not_strict() -> Result<()> {
        let dir = new_tempdir("ontoenv-permissions")?;
        setup!(&dir, {
            "fixtures/ont1.ttl" => "ont1.ttl",
            "fixtures/ont2.ttl" => "locked.ttl"
        });

        let locked_path = dir.path().join("locked.ttl");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&locked_path)?;

        let cfg = default_config(&dir);
        let env = OntoEnv::init(cfg, false)?;
        let files = env.find_files()?;
        // `find_files` canonicalizes discovered paths (resolving symlinks /
        // normalizing Windows path forms), so expected entries must be
        // canonicalized the same way to compare equal.
        let readable = OntologyLocation::File(ontoenv::ontology::canonicalize_file_path(
            &dir.path().join("ont1.ttl"),
        ));
        assert!(
            files.contains(&readable),
            "find_files should still collect readable entries"
        );
        let canonical_locked = ontoenv::ontology::canonicalize_file_path(&locked_path);
        assert!(
            !files.contains(&OntologyLocation::File(canonical_locked)),
            "locked files should be skipped when encountering sharing violations"
        );

        drop(lock);
        teardown(dir);
        Ok(())
    }
}

#[test]
fn test_ontoenv_scans() -> Result<()> {
    let dir = new_tempdir("ontoenv")?;
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
    let dir = new_tempdir("ontoenv")?;
    setup!(&dir, { "fixtures/ont1.ttl" => "ont1.ttl", 
                   "fixtures/ont2.ttl" => "ont2.ttl",
                   "fixtures/ont3.ttl" => "ont3.ttl",
                   "fixtures/ont4.ttl" => "ont4.ttl" });
    let cfg = Config::builder()
        .root(dir.path().into())
        .locations(vec![dir.path().into()])
        .offline(true)
        .build()?;
    let mut env = OntoEnv::init(cfg, false)?;
    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 4);
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_num_triples() -> Result<()> {
    let dir = new_tempdir("fileendings")?;
    setup!(&dir, {"fixtures/fileendings/model" => "model", 
                  "fixtures/fileendings/model.n3" => "model.n3",
                  "fixtures/fileendings/model.nt" => "model.nt",
                  "fixtures/fileendings/model.ttl" => "model.ttl",
                  "fixtures/fileendings/model.xml" => "model.xml"});
    let cfg1 = Config::builder()
        .root(dir.path().into())
        .locations(vec![dir.path().into()])
        .includes(&["*.n3"])
        .excludes(&[] as &[&str])
        .offline(true)
        .build()?;
    let mut env = OntoEnv::init(cfg1, false)?;
    env.update()?;
    assert_eq!(env.stats()?.num_graphs, 1);
    assert_eq!(env.stats()?.num_triples, 5);
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_update() -> Result<()> {
    let dir = new_tempdir("ontoenv")?;
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
    let dir = new_tempdir("ontoenv")?;
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
    let dir = new_tempdir("ontoenv")?;
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
    let dir = new_tempdir("ontoenv")?;
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
    let dir = new_tempdir("ontoenv")?;
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
    env.add(loc, Overwrite::Allow, RefreshStrategy::UseCache)?;
    assert_eq!(env.stats()?.num_graphs, 5);
    teardown(dir);
    Ok(())
}

#[test]
fn test_add_from_bytes_resolves_imports_and_dependency_graph_edges() -> Result<()> {
    let dir = new_tempdir("ontoenv_add_from_bytes_imports")?;
    let dep_path = dir.path().join("dep.ttl");
    let dep_iri = url::Url::from_file_path(&dep_path).unwrap().to_string();
    let root_iri = "http://example.com/root-bytes";

    fs::write(
        &dep_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{dep}> a owl:Ontology .\n<{dep}#entity> <http://example.com/p> \"dep\" .",
            dep = dep_iri
        ),
    )?;

    let root_bytes = format!(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{root}> a owl:Ontology ;\n  owl:imports <{dep}> .\n<{root}#entity> <http://example.com/p> \"root\" .",
        root = root_iri,
        dep = dep_iri
    )
    .into_bytes();

    let cfg = Config::builder()
        .root(dir.path().into())
        .locations(vec![])
        .strict(false)
        .offline(true)
        .temporary(true)
        .build()?;
    let mut env = OntoEnv::init(cfg, true)?;

    let root_id = env.add_from_bytes(
        OntologyLocation::InMemory {
            identifier: "urn:ontoenv:root-bytes".to_string(),
        },
        root_bytes,
        Some(RdfFormat::Turtle),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;
    let dep_id = env
        .resolve(ResolveTarget::Graph(NamedNodeRef::new(&dep_iri)?.into()))
        .expect("dependency ontology should resolve");

    let closure = env.get_closure(&root_id, -1)?;
    assert_eq!(closure.len(), 2, "closure should include root and import");
    assert!(
        closure.contains(&dep_id),
        "closure should include dependency"
    );

    let dep_node = NamedNode::new(dep_iri.clone())?;
    let importers = env.get_importers(&dep_node)?;
    assert!(
        importers.iter().any(|id| id == &root_id),
        "dependency should have root as importer"
    );

    let import_paths = env.get_import_paths(&dep_node)?;
    assert!(
        import_paths
            .iter()
            .any(|path| path.len() == 2 && path[0] == root_id && path[1] == dep_id),
        "dependency path should contain root -> dependency"
    );

    teardown(dir);
    Ok(())
}

#[test]
fn test_add_from_bytes_matches_file_add_closure() -> Result<()> {
    let dir = new_tempdir("ontoenv_add_from_bytes_parity")?;
    let dep_path = dir.path().join("dep.ttl");
    let root_path = dir.path().join("root.ttl");
    let dep_iri = url::Url::from_file_path(&dep_path).unwrap().to_string();
    let root_iri = "http://example.com/root-parity";
    let root_ttl = format!(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{root}> a owl:Ontology ;\n  owl:imports <{dep}> .\n<{root}#entity> <http://example.com/p> \"root\" .",
        root = root_iri,
        dep = dep_iri
    );

    fs::write(
        &dep_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{dep}> a owl:Ontology .\n<{dep}#entity> <http://example.com/p> \"dep\" .",
            dep = dep_iri
        ),
    )?;
    fs::write(&root_path, &root_ttl)?;

    let cfg = Config::builder()
        .root(dir.path().into())
        .locations(vec![])
        .strict(false)
        .offline(true)
        .temporary(true)
        .build()?;
    let mut env_bytes = OntoEnv::init(cfg.clone(), true)?;
    let mut env_file = OntoEnv::init(cfg, true)?;

    let bytes_id = env_bytes.add_from_bytes(
        OntologyLocation::InMemory {
            identifier: "urn:ontoenv:parity-root".to_string(),
        },
        root_ttl.as_bytes().to_vec(),
        Some(RdfFormat::Turtle),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;
    let file_id = env_file.add(
        OntologyLocation::File(root_path),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;

    let bytes_closure = env_bytes
        .get_closure(&bytes_id, -1)?
        .iter()
        .map(|id| id.to_uri_string())
        .collect::<std::collections::HashSet<String>>();
    let file_closure = env_file
        .get_closure(&file_id, -1)?
        .iter()
        .map(|id| id.to_uri_string())
        .collect::<std::collections::HashSet<String>>();
    assert_eq!(
        bytes_closure, file_closure,
        "byte-backed and file-backed adds should produce identical closure members"
    );

    let dep_node = NamedNode::new(dep_iri)?;
    let bytes_paths = env_bytes.get_import_paths(&dep_node)?;
    let file_paths = env_file.get_import_paths(&dep_node)?;
    let bytes_path_names = bytes_paths
        .into_iter()
        .map(|path| {
            path.into_iter()
                .map(|id| id.to_uri_string())
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::HashSet<Vec<String>>>();
    let file_path_names = file_paths
        .into_iter()
        .map(|path| {
            path.into_iter()
                .map(|id| id.to_uri_string())
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::HashSet<Vec<String>>>();
    assert_eq!(
        bytes_path_names, file_path_names,
        "byte-backed and file-backed adds should build the same dependency edges"
    );

    teardown(dir);
    Ok(())
}

#[test]
fn test_add_from_bytes_use_cache_reloads_when_bytes_change() -> Result<()> {
    let dir = new_tempdir("ontoenv_add_from_bytes_cache_refresh")?;
    let mut env = cached_env(&dir)?;
    let location = OntologyLocation::InMemory {
        identifier: "urn:ontoenv:cache-refresh-root".to_string(),
    };

    let bytes_v1 = b"@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.com/cache-root> a owl:Ontology .\n<http://example.com/cache-root#s> <http://example.com/p> \"v1\" .".to_vec();
    let id_v1 = env.add_from_bytes(
        location.clone(),
        bytes_v1,
        Some(RdfFormat::Turtle),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;
    let hash_v1 = env
        .get_ontology(&id_v1)?
        .content_hash()
        .expect("content hash must be set for byte-backed add")
        .to_string();

    let bytes_v2 = b"@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.com/cache-root> a owl:Ontology .\n<http://example.com/cache-root#s> <http://example.com/p> \"v2\" .".to_vec();
    let id_v2 = env.add_from_bytes(
        location,
        bytes_v2,
        Some(RdfFormat::Turtle),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;
    assert_eq!(id_v1, id_v2, "root identifier should remain stable");

    let hash_v2 = env
        .get_ontology(&id_v2)?
        .content_hash()
        .expect("content hash must be set for byte-backed add")
        .to_string();
    assert_ne!(
        hash_v1, hash_v2,
        "changed bytes should refresh cached ontology metadata"
    );

    let refreshed_graph = env.get_graph(&id_v2)?;
    let rendered_triples: Vec<String> =
        refreshed_graph.iter().map(|t| format!("{:?}", t)).collect();
    assert!(
        rendered_triples.iter().any(|t| t.contains("\"v2\"")),
        "refreshed graph should contain updated triple"
    );
    assert!(
        !rendered_triples.iter().any(|t| t.contains("\"v1\"")),
        "refreshed graph should not retain stale triple from prior bytes"
    );

    teardown(dir);
    Ok(())
}

#[test]
fn test_add_from_bytes_strict_errors_on_missing_import() -> Result<()> {
    let dir = new_tempdir("ontoenv_add_from_bytes_strict_missing")?;
    let missing_path = dir.path().join("missing.ttl");
    let missing_iri = url::Url::from_file_path(&missing_path).unwrap().to_string();
    let root_bytes = format!(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.com/root-strict> a owl:Ontology ;\n  owl:imports <{missing}> .",
        missing = missing_iri
    )
    .into_bytes();

    let cfg = Config::builder()
        .root(dir.path().into())
        .locations(vec![])
        .strict(true)
        .offline(true)
        .temporary(true)
        .build()?;
    let mut env = OntoEnv::init(cfg, true)?;

    let result = env.add_from_bytes(
        OntologyLocation::InMemory {
            identifier: "urn:ontoenv:strict-root".to_string(),
        },
        root_bytes,
        Some(RdfFormat::Turtle),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    );
    assert!(result.is_err(), "strict mode should fail on missing import");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Failed to load ontology"),
        "error should report failed import loading"
    );
    assert!(
        env.ontologies().is_empty(),
        "failed strict add should not register ontology metadata"
    );

    teardown(dir);
    Ok(())
}

#[test]
fn test_add_from_bytes_non_strict_skips_missing_import() -> Result<()> {
    let dir = new_tempdir("ontoenv_add_from_bytes_non_strict_missing")?;
    let missing_path = dir.path().join("missing.ttl");
    let missing_iri = url::Url::from_file_path(&missing_path).unwrap().to_string();
    let root_bytes = format!(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.com/root-non-strict> a owl:Ontology ;\n  owl:imports <{missing}> .",
        missing = missing_iri
    )
    .into_bytes();

    let cfg = Config::builder()
        .root(dir.path().into())
        .locations(vec![])
        .strict(false)
        .offline(true)
        .temporary(false)
        .build()?;
    let mut env = OntoEnv::init(cfg, true)?;

    let root_id = env.add_from_bytes(
        OntologyLocation::InMemory {
            identifier: "urn:ontoenv:non-strict-root".to_string(),
        },
        root_bytes,
        Some(RdfFormat::Turtle),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;
    let closure = env.get_closure(&root_id, -1)?;
    assert_eq!(closure.len(), 1, "missing import should be skipped");
    assert_eq!(
        env.missing_imports()
            .into_iter()
            .filter(|n| n.as_str() == missing_iri)
            .count(),
        1,
        "missing import should be tracked"
    );
    let pending = dir
        .path()
        .join(".ontoenv")
        .join(ontoenv::catalog::PENDING_FILE);
    assert!(
        !pending.exists(),
        "a tolerated non-strict import failure must not leave a recovery marker"
    );
    drop(env);
    let reopened = OntoEnv::load_from_directory(dir.path().to_path_buf(), false)?;
    assert_eq!(
        reopened.get_closure(&root_id, -1)?.len(),
        1,
        "the successfully committed partial environment should reopen normally"
    );

    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_detect_updates() -> Result<()> {
    let dir = new_tempdir("ontoenv")?;
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
    let dir = new_tempdir("ontoenv")?;
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
    assert_eq!(updates.len(), 2);
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_dependency_closure() -> Result<()> {
    let dir = new_tempdir("ontoenv")?;
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
    let closure = env.get_closure(&ont_graph, -1).unwrap();
    assert_eq!(closure.len(), 19);
    teardown(dir);
    Ok(())
}

#[test]
fn test_ontoenv_dag_structure() -> Result<()> {
    let dir = new_tempdir("ontoenv")?;
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
    let closure = env.get_closure(&ont_graph, -1).unwrap();
    assert_eq!(closure.len(), 2);
    let root = closure[0].name();
    let union = env.get_union_graph(&closure, root, None, None)?;
    assert_eq!(union.len(), 4);
    let union = env.get_union_graph(&closure, root, None, Some(false))?;
    assert_eq!(union.len(), 5);

    // ont3 => {ont3, ont2, ont1}
    let ont3 = NamedNodeRef::new("http://example.org/ontology3")?;
    let ont_graph = env.resolve(ResolveTarget::Graph(ont3.into())).unwrap();
    let closure = env.get_closure(&ont_graph, -1).unwrap();
    assert_eq!(closure.len(), 3);
    let root = closure[0].name();
    let union = env.get_union_graph(&closure, root, None, None)?;
    assert_eq!(union.len(), 5);
    let union = env.get_union_graph(&closure, root, None, Some(false))?;
    assert_eq!(union.len(), 8);

    // ont5 => {ont5, ont4, ont3, ont2, ont1}
    let ont5 = NamedNodeRef::new("http://example.org/ontology5")?;
    let ont_graph = env.resolve(ResolveTarget::Graph(ont5.into())).unwrap();
    let closure = env.get_closure(&ont_graph, -1).unwrap();
    assert_eq!(closure.len(), 5);
    let root = closure[0].name();
    let union = env.get_union_graph(&closure, root, None, None)?;
    assert_eq!(union.len(), 7);
    let union = env.get_union_graph(&closure, root, None, Some(false))?;
    // print the union
    assert_eq!(union.len(), 14);

    // check recursion depths
    let closure = env.get_closure(&ont_graph, 0).unwrap();
    assert_eq!(closure.len(), 1);
    let closure_names: std::collections::HashSet<String> =
        closure.iter().map(|ont| ont.name().to_string()).collect();
    assert!(closure_names.contains("<http://example.org/ontology5>"));

    let closure = env.get_closure(&ont_graph, 1).unwrap();
    assert_eq!(closure.len(), 4); // ont5, ont4, ont3, ont2
    let closure_names: std::collections::HashSet<String> =
        closure.iter().map(|ont| ont.name().to_string()).collect();
    assert!(closure_names.contains("<http://example.org/ontology5>"));
    assert!(closure_names.contains("<http://example.org/ontology4>"));
    assert!(closure_names.contains("<http://example.org/ontology3>"));
    assert!(closure_names.contains("<http://example.org/ontology2>"));

    let closure = env.get_closure(&ont_graph, -1).unwrap();
    assert_eq!(closure.len(), 5); // ont5, ont4, ont3, ont2, ont1
    let closure_names: std::collections::HashSet<String> =
        closure.iter().map(|ont| ont.name().to_string()).collect();
    assert!(closure_names.contains("<http://example.org/ontology5>"));
    assert!(closure_names.contains("<http://example.org/ontology4>"));
    assert!(closure_names.contains("<http://example.org/ontology3>"));
    assert!(closure_names.contains("<http://example.org/ontology2>"));
    assert!(closure_names.contains("<http://example.org/ontology1>"));

    Ok(())
}

// === Initialization Tests Translated from Python ===

#[test]
fn test_init_with_config_new_dir() -> Result<()> {
    let dir = new_tempdir("ontoenv_init_new")?;
    let env_path = dir.path().join("new_env");
    // Ensure the directory does not exist initially
    assert!(!env_path.exists());

    let cfg = Config::builder()
        .root(env_path.clone())
        .locations(vec![env_path.clone()])
        .includes(&["*.ttl"])
        .excludes(&[] as &[&str])
        .build()?;

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
    let dir = new_tempdir("ontoenv_init_empty")?;
    let env_path = dir.path().join("empty_env");
    std::fs::create_dir(&env_path)?;
    assert!(env_path.is_dir());
    assert!(std::fs::read_dir(&env_path)?.next().is_none()); // Check empty

    let cfg = Config::builder()
        .root(env_path.clone())
        .locations(vec![env_path.clone()])
        .includes(&["*.ttl"])
        .excludes(&[] as &[&str])
        .build()?;

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
    let dir = new_tempdir("ontoenv_load_existing")?;
    let env_path = dir.path().join("existing_env");
    std::fs::create_dir(&env_path)?;

    // Create a dummy environment first
    let cfg = Config::builder()
        .root(env_path.clone())
        .locations(vec![env_path.clone()])
        .includes(&["*.ttl"])
        .excludes(&[] as &[&str])
        .build()?;
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
fn test_lazy_flush_preserves_unloaded_graphs() -> Result<()> {
    let dir = new_tempdir("ontoenv_lazy_flush")?;
    setup!(&dir, {"fixtures/rdftest/ontology1.ttl" => "ontology1.ttl",
                  "fixtures/rdftest/ontology2.ttl" => "ontology2.ttl"});

    let cfg = default_config(&dir);
    let mut env = OntoEnv::init(cfg, false)?;
    env.update()?;
    env.flush()?;
    env.save_to_directory()?;
    drop(env);

    let mut loaded_env = OntoEnv::load_from_directory(dir.path().into(), false)?;
    let ont2 = NamedNodeRef::new("http://example.org/ontology2")?;
    let ont2_id = loaded_env
        .resolve(ResolveTarget::Graph(ont2.into()))
        .unwrap();
    let _ = loaded_env.get_graph(&ont2_id)?;

    let extra_path = dir.path().join("ontology_extra.ttl");
    std::fs::write(
        &extra_path,
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.org/extra> a owl:Ontology .",
    )?;
    loaded_env.add(
        OntologyLocation::File(extra_path),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )?;
    loaded_env.flush()?;
    drop(loaded_env);

    let reloaded_env = OntoEnv::load_from_directory(dir.path().into(), false)?;
    let ont1 = NamedNodeRef::new("http://example.org/ontology1")?;
    let ont1_id = reloaded_env
        .resolve(ResolveTarget::Graph(ont1.into()))
        .unwrap();
    let graph = reloaded_env.get_graph(&ont1_id)?;
    assert!(graph.iter().next().is_some());

    teardown(dir);
    Ok(())
}

#[test]
fn test_init_recreate_existing_dir() -> Result<()> {
    let dir = new_tempdir("ontoenv_recreate")?;
    let env_path = dir.path().join("recreate_env");
    std::fs::create_dir(&env_path)?;

    // Create a dummy environment first
    let cfg = Config::builder()
        .root(env_path.clone())
        .locations(vec![env_path.clone()])
        .includes(&["*.ttl"])
        .excludes(&[] as &[&str])
        .build()?;
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
    let dir = new_tempdir("ontoenv_readonly")?;
    let env_path = dir.path().join("readonly_env");
    std::fs::create_dir(&env_path)?;

    // Create a dummy environment first
    let cfg = Config::builder()
        .root(env_path.clone())
        .locations(vec![env_path.clone()])
        .includes(&["*.ttl"])
        .excludes(&[] as &[&str])
        .build()?;
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
    let add_result = loaded_env.add(location, Overwrite::Preserve, RefreshStrategy::UseCache);

    assert!(add_result.is_err());
    // Check if the error message indicates read-only restriction
    // Note: The exact error might depend on the GraphIO implementation details.
    // Assuming ReadOnlyPersistentGraphIO::add returns a specific error.
    // If GraphIO trait doesn't have 'add', this test might need adjustment based on how OntoEnv handles it.
    // Let's assume GraphIO has 'add' and ReadOnly returns an error like below.
    let err_string = add_result.unwrap_err().to_string();
    assert!(err_string.contains("Cannot add to read-only store"));

    teardown(dir);
    Ok(())
}

#[test]
fn test_init_path_no_env_error() -> Result<()> {
    let dir = new_tempdir("ontoenv_path_no_env")?;
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
    let dir = new_tempdir("ontoenv_temporary")?;
    let env_path = dir.path().join("temp_env_root");
    // Temporary envs shouldn't persist to disk relative to root

    let cfg = Config::builder()
        .root(env_path.clone())
        .locations(vec![env_path.clone()])
        .includes(&["*.ttl"])
        .excludes(&[] as &[&str])
        .temporary(true)
        .build()?;

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

    let add_result = env.add(location, Overwrite::Preserve, RefreshStrategy::UseCache);
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

#[test]
fn test_cached_add_skips_unchanged_file() -> Result<()> {
    let dir = new_tempdir("ontoenv_cached_skip")?;
    let ttl_path = dir.path().join("cached.ttl");
    fs::write(
        &ttl_path,
        "<urn:cached> a <http://www.w3.org/2002/07/owl#Ontology> .",
    )?;

    let mut env = cached_env(&dir)?;
    let location = OntologyLocation::File(ttl_path.clone());
    let id = env.add(
        location.clone(),
        Overwrite::Preserve,
        RefreshStrategy::UseCache,
    )?;
    let first_updated = env
        .ontologies()
        .get(&id)
        .and_then(|ont| ont.last_updated)
        .expect("last_updated set");
    assert_eq!(env.stats()?.num_ontologies, 1);

    thread::sleep(Duration::from_secs(1));

    let reused_id = env.add(
        location.clone(),
        Overwrite::Preserve,
        RefreshStrategy::UseCache,
    )?;
    let reused_updated = env
        .ontologies()
        .get(&reused_id)
        .and_then(|ont| ont.last_updated)
        .expect("last_updated still set");

    assert_eq!(id, reused_id);
    assert_eq!(first_updated, reused_updated);
    assert_eq!(env.stats()?.num_ontologies, 1);

    drop(env);
    teardown(dir);
    Ok(())
}

#[test]
fn test_cached_add_reloads_on_file_change() -> Result<()> {
    let dir = new_tempdir("ontoenv_cached_reload")?;
    let ttl_path = dir.path().join("cached_reload.ttl");
    fs::write(
        &ttl_path,
        "<urn:cached_reload> a <http://www.w3.org/2002/07/owl#Ontology> .",
    )?;

    let mut env = cached_env(&dir)?;
    let location = OntologyLocation::File(ttl_path.clone());
    let id = env.add(
        location.clone(),
        Overwrite::Preserve,
        RefreshStrategy::UseCache,
    )?;
    let first_updated = env
        .ontologies()
        .get(&id)
        .and_then(|ont| ont.last_updated)
        .expect("last_updated set");

    thread::sleep(Duration::from_secs(1));

    fs::write(
        &ttl_path,
        "<urn:cached_reload> a <http://www.w3.org/2002/07/owl#Ontology> .\n<urn:cached_reload> <http://example.com/p> \"updated\" .",
    )?;

    let refreshed_id = env.add(
        location.clone(),
        Overwrite::Preserve,
        RefreshStrategy::UseCache,
    )?;
    let refreshed_updated = env
        .ontologies()
        .get(&refreshed_id)
        .and_then(|ont| ont.last_updated)
        .expect("last_updated set after refresh");

    assert_eq!(id, refreshed_id);
    assert!(refreshed_updated > first_updated);

    drop(env);
    teardown(dir);
    Ok(())
}

#[test]
fn test_cached_add_force_refreshes() -> Result<()> {
    let dir = new_tempdir("ontoenv_cached_force")?;
    let ttl_path = dir.path().join("cached_force.ttl");
    fs::write(
        &ttl_path,
        "<urn:cached_force> a <http://www.w3.org/2002/07/owl#Ontology> .",
    )?;

    let mut env = cached_env(&dir)?;
    let location = OntologyLocation::File(ttl_path.clone());
    let id = env.add(
        location.clone(),
        Overwrite::Preserve,
        RefreshStrategy::UseCache,
    )?;
    let first_updated = env
        .ontologies()
        .get(&id)
        .and_then(|ont| ont.last_updated)
        .expect("last_updated set");

    thread::sleep(Duration::from_secs(1));

    let forced_id = env.add(
        location.clone(),
        Overwrite::Preserve,
        RefreshStrategy::Force,
    )?;
    let forced_updated = env
        .ontologies()
        .get(&forced_id)
        .and_then(|ont| ont.last_updated)
        .expect("last_updated set after force");

    assert_eq!(id, forced_id);
    assert!(forced_updated > first_updated);

    drop(env);
    teardown(dir);
    Ok(())
}

// ── rename tests ─────────────────────────────────────────────────────────────

fn in_memory_env() -> Result<OntoEnv> {
    let root = std::env::current_dir()?;
    let cfg = Config::builder()
        .root(root)
        .locations(vec![])
        .strict(false)
        .offline(true)
        .temporary(true)
        .build()?;
    OntoEnv::init(cfg, true)
}

fn add_bytes(
    env: &mut OntoEnv,
    id: &str,
    turtle: &str,
) -> Result<ontoenv::ontology::GraphIdentifier> {
    env.add_from_bytes(
        OntologyLocation::InMemory {
            identifier: id.to_string(),
        },
        turtle.as_bytes().to_vec(),
        Some(RdfFormat::Turtle),
        Overwrite::Allow,
        RefreshStrategy::UseCache,
    )
}

/// After rename the new IRI resolves in the env, the old IRI does not,
/// and the stored graph data carries the new IRI as the `owl:Ontology` subject.
#[test]
fn rename_updates_env_and_graph_data() -> Result<()> {
    let mut env = in_memory_env()?;

    add_bytes(
        &mut env,
        "urn:test:B",
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         <http://example.com/B> a owl:Ontology .\n",
    )?;

    let old_iri = NamedNode::new("http://example.com/B")?;
    let b_id = env
        .resolve(ResolveTarget::Graph(old_iri.clone()))
        .expect("B should be in env before rename");

    let new_iri = NamedNode::new("http://example.com/B-renamed")?;
    let new_id = env.rename_graph_iri(&b_id, new_iri.clone())?;

    // Old IRI removed, new IRI present.
    assert!(
        env.resolve(ResolveTarget::Graph(old_iri)).is_none(),
        "old IRI should be gone after rename"
    );
    assert!(
        env.resolve(ResolveTarget::Graph(new_iri.clone())).is_some(),
        "new IRI should be in env after rename"
    );
    assert_eq!(new_id.to_uri_string(), new_iri.as_str());

    // The stored graph has the new IRI as the owl:Ontology subject.
    let graph = env.io().get_graph(&new_id)?;
    let rdf_type = NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    let owl_ontology_node = NamedNodeRef::new_unchecked("http://www.w3.org/2002/07/owl#Ontology");

    let new_iri_as_subject = NamedOrBlankNodeRef::NamedNode(new_iri.as_ref());
    let has_new_declaration = graph
        .triples_for_subject(new_iri_as_subject)
        .any(|t| t.predicate == rdf_type && t.object == TermRef::NamedNode(owl_ontology_node));
    assert!(
        has_new_declaration,
        "graph should declare new IRI as owl:Ontology"
    );

    let old_iri_node = NamedNode::new("http://example.com/B")?;
    let old_iri_as_subject = NamedOrBlankNodeRef::NamedNode(old_iri_node.as_ref());
    let old_declaration_gone = graph
        .triples_for_subject(old_iri_as_subject)
        .next()
        .is_none();
    assert!(
        old_declaration_gone,
        "old IRI should have no triples as subject"
    );

    Ok(())
}

/// Renaming a node with downstream imports keeps those imports reachable in
/// the renamed node's own transitive closure.
#[test]
fn rename_preserves_downstream_closure() -> Result<()> {
    let mut env = in_memory_env()?;

    // C: standalone leaf
    add_bytes(
        &mut env,
        "urn:test:C",
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         <http://example.com/C> a owl:Ontology .\n",
    )?;

    // B imports C
    add_bytes(
        &mut env,
        "urn:test:B",
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         <http://example.com/B> a owl:Ontology ;\n\
           owl:imports <http://example.com/C> .\n",
    )?;

    let b_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(
            "http://example.com/B",
        )?))
        .expect("B should be in env");
    let c_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(
            "http://example.com/C",
        )?))
        .expect("C should be in env");

    // B's closure before rename: [B, C]
    let closure_before = env.get_closure(&b_id, -1)?;
    assert_eq!(closure_before.len(), 2);
    assert!(closure_before.contains(&c_id));

    // Rename B → B-renamed
    let new_b_iri = NamedNode::new("http://example.com/B-renamed")?;
    let b_new_id = env.rename_graph_iri(&b_id, new_b_iri)?;

    // Old B gone, B-renamed present.
    assert!(
        env.resolve(ResolveTarget::Graph(NamedNode::new(
            "http://example.com/B"
        )?))
        .is_none(),
        "old B should be gone after rename"
    );
    assert!(
        env.resolve(ResolveTarget::Graph(NamedNode::new(
            "http://example.com/B-renamed"
        )?))
        .is_some(),
        "B-renamed should be in env"
    );

    // B-renamed's closure still includes C, and does not include old B.
    let closure_after = env.get_closure(&b_new_id, -1)?;
    assert!(
        closure_after.contains(&c_id),
        "C should still be in B-renamed's closure"
    );
    assert!(
        !closure_after.contains(&b_id),
        "old B should not appear in B-renamed's closure"
    );
    assert_eq!(closure_after.len(), 2, "closure should be [B-renamed, C]");

    Ok(())
}

/// add_with_rename loads the root and its transitive imports. The returned
/// identifier carries the renamed IRI; imported ontologies are reachable
/// from the renamed root's closure.
#[test]
fn add_with_rename_closure_includes_imports() -> Result<()> {
    let dir = new_tempdir("ontoenv_add_with_rename_closure")?;

    // C: standalone leaf identified by its file URL
    let c_path = dir.path().join("C.ttl");
    let c_iri = url::Url::from_file_path(&c_path).unwrap().to_string();
    fs::write(
        &c_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{c}> a owl:Ontology .\n",
            c = c_iri
        ),
    )?;

    // B imports C (both identified by their file URLs)
    let b_path = dir.path().join("B.ttl");
    let b_iri = url::Url::from_file_path(&b_path).unwrap().to_string();
    fs::write(
        &b_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{b}> a owl:Ontology ; owl:imports <{c}> .\n",
            b = b_iri,
            c = c_iri,
        ),
    )?;

    // A imports B (also identified by file URL)
    let a_path = dir.path().join("A.ttl");
    let a_iri = url::Url::from_file_path(&a_path).unwrap().to_string();
    fs::write(
        &a_path,
        format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{a}> a owl:Ontology ; owl:imports <{b}> .\n",
            a = a_iri,
            b = b_iri,
        ),
    )?;

    let cfg = Config::builder()
        .root(dir.path().into())
        .locations(vec![])
        .strict(false)
        .offline(true)
        .temporary(true)
        .build()?;
    let mut env = OntoEnv::init(cfg, true)?;

    let new_a_iri = "http://example.com/A-canonical";
    let a_id = env.add_with_rename(
        OntologyLocation::File(a_path),
        Overwrite::Allow,
        RefreshStrategy::Force,
        NamedNode::new(new_a_iri)?,
    )?;

    // Root is registered under the new IRI.
    assert_eq!(a_id.to_uri_string(), new_a_iri);
    assert!(
        env.resolve(ResolveTarget::Graph(NamedNode::new(new_a_iri)?))
            .is_some(),
        "renamed A should be in env"
    );

    // B and C were loaded as transitive dependencies.
    let b_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(&b_iri)?))
        .expect("B should be in env as import of A");
    let c_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(&c_iri)?))
        .expect("C should be in env as transitive import");

    // Transitive closure of A-canonical contains B and C.
    let closure = env.get_closure(&a_id, -1)?;
    assert!(
        closure.contains(&b_id),
        "B should be in A-canonical's closure"
    );
    assert!(
        closure.contains(&c_id),
        "C should be in A-canonical's closure"
    );
    assert_eq!(closure.len(), 3, "closure should be [A-canonical, B, C]");

    teardown(dir);
    Ok(())
}

/// Rename rewrites every occurrence of the old IRI inside the graph:
///
/// - subject position: `<old> rdf:type owl:Ontology`, `<old> owl:imports <C>`,
///   `<old> sh:prefixes <old>`, `<old> sh:declare _:decl`,
///   `<old> owl:versionIRI <old>`
/// - object position: `<NamedShape> sh:prefixes <old>` (subject is NOT old IRI),
///   self-referential `owl:versionIRI` and `sh:prefixes`
///
/// After rename none of the old IRI must appear anywhere in the graph.
#[test]
fn rename_rewrites_sh_prefixes_owl_imports_and_version_iri() -> Result<()> {
    let old_iri = "http://example.com/B";
    let new_iri = "http://example.com/B-new";
    let c_iri = "http://example.com/C";
    let shape_iri = "http://example.com/MyShape";

    // Construct a rich turtle document that exercises every rewrite position:
    //
    //   old:B  rdf:type            owl:Ontology
    //   old:B  owl:versionIRI      old:B          ← self-referential (subject + object)
    //   old:B  owl:imports         C              ← subject rewrite; C object preserved
    //   old:B  sh:prefixes         old:B          ← self-referential (subject + object)
    //   old:B  sh:declare          _:decl         ← subject rewrite; blank-node object unchanged
    //   shape  sh:prefixes         old:B          ← object-only rewrite (subject ≠ old)
    let turtle = format!(
        "@prefix owl:  <http://www.w3.org/2002/07/owl#> .\n\
         @prefix sh:   <http://www.w3.org/ns/shacl#> .\n\
         @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
         \n\
         <{old}> a owl:Ontology ;\n\
             owl:versionIRI <{old}> ;\n\
             owl:imports <{c}> ;\n\
             sh:prefixes <{old}> ;\n\
             sh:declare [ sh:prefix \"ex\" ; sh:namespace <http://example.com/> ] .\n\
         \n\
         <{shape}> sh:prefixes <{old}> .\n",
        old = old_iri,
        c = c_iri,
        shape = shape_iri,
    );

    let mut env = in_memory_env()?;
    add_bytes(&mut env, "urn:test:B-rich", &turtle)?;

    let b_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(old_iri)?))
        .expect("B should be loaded");
    env.rename_graph_iri(&b_id, NamedNode::new(new_iri)?)?;

    let new_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(new_iri)?))
        .expect("B-new should be in env after rename");
    let graph = env.io().get_graph(&new_id)?;

    // Old IRI must not appear as a subject or as the object of any predicate
    // other than owl:versionIRI (version identifiers are intentionally preserved).
    let owl_version_iri_pred = "http://www.w3.org/2002/07/owl#versionIRI";
    for t in graph.iter() {
        if let oxigraph::model::NamedOrBlankNodeRef::NamedNode(nn) = t.subject {
            assert_ne!(
                nn.as_str(),
                old_iri,
                "old IRI must not appear as subject: {t}"
            );
        }
        if t.predicate.as_str() != owl_version_iri_pred {
            if let TermRef::NamedNode(nn) = t.object {
                assert_ne!(
                    nn.as_str(),
                    old_iri,
                    "old IRI must not appear as object (predicate={}): {t}",
                    t.predicate
                );
            }
        }
    }

    // New IRI must appear in every expected position.
    let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    let owl_ontology = "http://www.w3.org/2002/07/owl#Ontology";
    let owl_version_iri = "http://www.w3.org/2002/07/owl#versionIRI";
    let owl_imports = "http://www.w3.org/2002/07/owl#imports";
    let sh_prefixes = "http://www.w3.org/ns/shacl#prefixes";

    let has_triple = |s: Option<&str>, p: &str, o: Option<&str>| {
        graph.iter().any(|t| {
            let subj_ok = s.is_none_or(|expected| match t.subject {
                oxigraph::model::NamedOrBlankNodeRef::NamedNode(nn) => nn.as_str() == expected,
                _ => false,
            });
            let pred_ok = t.predicate.as_str() == p;
            let obj_ok = o.is_none_or(|expected| match t.object {
                TermRef::NamedNode(nn) => nn.as_str() == expected,
                _ => false,
            });
            subj_ok && pred_ok && obj_ok
        })
    };

    assert!(
        has_triple(Some(new_iri), rdf_type, Some(owl_ontology)),
        "<new> rdf:type owl:Ontology should be present"
    );
    assert!(
        has_triple(Some(new_iri), owl_version_iri, Some(old_iri)),
        "<new> owl:versionIRI <old> should be present (subject rewritten, version value preserved)"
    );
    assert!(
        !has_triple(Some(new_iri), owl_version_iri, Some(new_iri)),
        "<new> owl:versionIRI <new> must NOT appear — version IRI is not rewritten"
    );
    assert!(
        has_triple(Some(new_iri), owl_imports, Some(c_iri)),
        "<new> owl:imports <C> should be present (C object preserved)"
    );
    assert!(
        has_triple(Some(new_iri), sh_prefixes, Some(new_iri)),
        "<new> sh:prefixes <new> should be present (self-ref rewritten on both sides)"
    );
    assert!(
        has_triple(Some(shape_iri), sh_prefixes, Some(new_iri)),
        "<Shape> sh:prefixes <new> should be present (object-only rewrite)"
    );

    // C must NOT have been rewritten (it is a different named node).
    assert!(
        has_triple(Some(new_iri), owl_imports, Some(c_iri)),
        "owl:imports target <C> must remain unchanged"
    );

    Ok(())
}

/// Three-node chain A → B → C built with in-memory ontologies.
/// After renaming the middle node (B → B-new):
///  - B-old is absent from the environment.
///  - B-new is present, and its own closure still reaches C.
///  - No closure of any remaining ontology contains B-old's IRI.
#[test]
fn rename_middle_node_dep_graph_updated() -> Result<()> {
    let mut env = in_memory_env()?;

    let a_iri = "http://example.com/A";
    let b_iri = "http://example.com/B";
    let c_iri = "http://example.com/C";
    let b_new_iri = "http://example.com/B-new";

    add_bytes(
        &mut env,
        "urn:test:C",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{c}> a owl:Ontology .\n",
            c = c_iri
        ),
    )?;
    add_bytes(
        &mut env,
        "urn:test:B",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{b}> a owl:Ontology ; owl:imports <{c}> .\n",
            b = b_iri,
            c = c_iri,
        ),
    )?;
    add_bytes(
        &mut env,
        "urn:test:A",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{a}> a owl:Ontology ; owl:imports <{b}> .\n",
            a = a_iri,
            b = b_iri,
        ),
    )?;

    let b_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(b_iri)?))
        .expect("B should be in env before rename");
    let c_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(c_iri)?))
        .expect("C should be in env");
    let a_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(a_iri)?))
        .expect("A should be in env");

    // Sanity check: full chain reachable before rename.
    let closure_before = env.get_closure(&a_id, -1)?;
    assert_eq!(
        closure_before.len(),
        3,
        "A's closure should be [A, B, C] before rename"
    );

    // ── rename B ──────────────────────────────────────────────────────────────
    let b_new_id = env.rename_graph_iri(&b_id, NamedNode::new(b_new_iri)?)?;

    // B-old gone, B-new present.
    assert!(
        env.resolve(ResolveTarget::Graph(NamedNode::new(b_iri)?))
            .is_none(),
        "B-old should be absent after rename"
    );
    assert!(
        env.resolve(ResolveTarget::Graph(NamedNode::new(b_new_iri)?))
            .is_some(),
        "B-new should be present after rename"
    );
    assert_eq!(b_new_id.to_uri_string(), b_new_iri);

    // B-new still reaches C through its own imports.
    let b_new_closure = env.get_closure(&b_new_id, -1)?;
    assert!(
        b_new_closure.contains(&c_id),
        "C should still be reachable from B-new"
    );
    assert_eq!(
        b_new_closure.len(),
        2,
        "B-new's closure should be [B-new, C]"
    );

    // B-old's IRI does not appear in any ontology's closure.
    for id in env.ontologies().keys() {
        let closure = env.get_closure(id, -1).unwrap_or_default();
        assert!(
            !closure.iter().any(|g| g.to_uri_string() == b_iri),
            "B-old IRI ({b_iri}) should not appear in closure of {}",
            id.to_uri_string()
        );
    }

    Ok(())
}

// ── alias tests ──────────────────────────────────────────────────────────────

/// Add an alias and verify it resolves to the same graph as the canonical IRI.
#[test]
fn alias_routes_to_canonical_graph() -> Result<()> {
    let mut env = in_memory_env()?;

    let canonical_iri = "http://example.com/ont";
    let alias_iri = "http://example.com/ont-alias";

    add_bytes(
        &mut env,
        "urn:test:ont",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{}> a owl:Ontology .\n",
            canonical_iri
        ),
    )?;

    // Add alias
    env.add_alias(alias_iri, canonical_iri)?;

    // Both IRI and alias resolve to the same graph
    let canonical_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(canonical_iri)?))
        .expect("canonical IRI should be in env");
    let alias_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(alias_iri)?))
        .expect("alias should be in env");

    assert_eq!(
        canonical_id, alias_id,
        "alias should resolve to same graph as canonical"
    );

    // get_graph should work with both
    let canonical_graph = env.get_graph(&canonical_id)?;
    let alias_graph = env.get_graph(&alias_id)?;

    assert_eq!(
        canonical_graph.iter().count(),
        alias_graph.iter().count(),
        "both should return same number of triples"
    );

    Ok(())
}

/// Remove an alias and verify it no longer resolves.
#[test]
fn remove_alias_stops_resolving() -> Result<()> {
    let mut env = in_memory_env()?;

    let canonical_iri = "http://example.com/ont";
    let alias_iri = "http://example.com/ont-alias";

    add_bytes(
        &mut env,
        "urn:test:ont",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{}> a owl:Ontology .\n",
            canonical_iri
        ),
    )?;

    env.add_alias(alias_iri, canonical_iri)?;

    // Verify alias works
    assert!(
        env.resolve(ResolveTarget::Graph(NamedNode::new(alias_iri)?))
            .is_some(),
        "alias should resolve before removal"
    );

    // Remove alias
    env.remove_alias(alias_iri)?;

    // Alias no longer resolves
    assert!(
        env.resolve(ResolveTarget::Graph(NamedNode::new(alias_iri)?))
            .is_none(),
        "alias should not resolve after removal"
    );

    // Canonical IRI still works
    assert!(
        env.resolve(ResolveTarget::Graph(NamedNode::new(canonical_iri)?))
            .is_some(),
        "canonical IRI should still work"
    );

    Ok(())
}

/// get_aliases_for returns all aliases for a canonical IRI.
#[test]
fn get_aliases_for_returns_all_aliases() -> Result<()> {
    let mut env = in_memory_env()?;

    let canonical_iri = "http://example.com/ont";
    let alias1_iri = "http://example.com/ont-alias1";
    let alias2_iri = "http://example.com/ont-alias2";

    add_bytes(
        &mut env,
        "urn:test:ont",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{}> a owl:Ontology .\n",
            canonical_iri
        ),
    )?;

    env.add_alias(alias1_iri, canonical_iri)?;
    env.add_alias(alias2_iri, canonical_iri)?;

    let aliases = env.get_aliases_for(canonical_iri);
    assert_eq!(aliases.len(), 2, "should have 2 aliases");
    assert!(
        aliases.contains(&alias1_iri.to_string()),
        "should contain alias1"
    );
    assert!(
        aliases.contains(&alias2_iri.to_string()),
        "should contain alias2"
    );

    Ok(())
}

/// Resolving a non-existent alias returns None.
#[test]
fn resolve_nonexistent_alias_returns_none() -> Result<()> {
    let mut env = in_memory_env()?;

    let canonical_iri = "http://example.com/ont";
    add_bytes(
        &mut env,
        "urn:test:ont",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{}> a owl:Ontology .\n",
            canonical_iri
        ),
    )?;

    // Try to resolve an alias that was never added
    assert!(
        env.resolve_alias("http://example.com/nonexistent-alias")
            .is_none(),
        "non-existent alias should return None"
    );

    Ok(())
}

/// Aliases only point to canonical IRIs, not other aliases.
#[test]
fn aliases_point_only_to_canonical_no_chains() -> Result<()> {
    let mut env = in_memory_env()?;

    let canonical_iri = "http://example.com/ont";
    let alias1_iri = "http://example.com/ont-alias1";
    let alias2_iri = "http://example.com/ont-alias2";

    add_bytes(
        &mut env,
        "urn:test:ont",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{}> a owl:Ontology .\n",
            canonical_iri
        ),
    )?;

    // Add alias1 pointing to canonical
    env.add_alias(alias1_iri, canonical_iri)?;

    // Try to add alias2 pointing to alias1 (should fail - alias1 is not canonical)
    let result = env.add_alias(alias2_iri, alias1_iri);
    assert!(result.is_err(), "alias chain should be rejected");

    // Verify alias2 was not added
    assert!(
        env.resolve_alias(alias2_iri).is_none(),
        "alias chain should not be created"
    );

    Ok(())
}

/// is_canonical_iri correctly identifies canonical IRIs vs aliases.
#[test]
fn is_canonical_iri_works_correctly() -> Result<()> {
    let mut env = in_memory_env()?;

    let canonical_iri = "http://example.com/ont";
    let alias_iri = "http://example.com/ont-alias";

    add_bytes(
        &mut env,
        "urn:test:ont",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{}> a owl:Ontology .\n",
            canonical_iri
        ),
    )?;

    // Before adding alias
    assert!(
        env.is_canonical_iri(canonical_iri),
        "canonical IRI should be canonical"
    );
    assert!(
        !env.is_canonical_iri(alias_iri),
        "non-existent alias should not be canonical"
    );

    // Add alias
    env.add_alias(alias_iri, canonical_iri)?;

    // After adding alias
    assert!(
        env.is_canonical_iri(canonical_iri),
        "canonical IRI should still be canonical"
    );
    assert!(
        !env.is_canonical_iri(alias_iri),
        "alias should not be canonical"
    );

    Ok(())
}

/// Aliases work with get_closure and other operations.
#[test]
fn alias_works_with_closure_operations() -> Result<()> {
    let mut env = in_memory_env()?;

    let canonical_iri = "http://example.com/ont";
    let alias_iri = "http://example.com/ont-alias";
    let imported_iri = "http://example.com/imported";

    // Add imported ontology
    add_bytes(
        &mut env,
        "urn:test:imported",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{}> a owl:Ontology .\n",
            imported_iri
        ),
    )?;

    // Add ontology that imports the imported one
    add_bytes(
        &mut env,
        "urn:test:ont",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology ; owl:imports <{}> .\n",
            canonical_iri, imported_iri
        ),
    )?;

    // Add alias
    env.add_alias(alias_iri, canonical_iri)?;

    // get_closure should work with both canonical and alias
    let canonical_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(canonical_iri)?))
        .expect("canonical IRI should be in env");
    let alias_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(alias_iri)?))
        .expect("alias should be in env");

    let canonical_closure = env.get_closure(&canonical_id, -1)?;
    let alias_closure = env.get_closure(&alias_id, -1)?;

    assert_eq!(
        canonical_closure.len(),
        alias_closure.len(),
        "closure should be same for canonical and alias"
    );

    Ok(())
}

/// Test that aliases don't cause duplicates when computing closure.
/// When an ontology is reached via both its canonical IRI and an alias,
/// it should only appear once in the closure.
#[test]
fn alias_deduplication_in_closure() -> Result<()> {
    let mut env = in_memory_env()?;

    let ont_a = "http://example.com/A";
    let ont_b = "http://example.com/B";
    let ont_b_alias = "http://example.com/B-alias";
    let ont_c = "http://example.com/C";

    // A imports B and C
    add_bytes(
        &mut env,
        "urn:test:A",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology ; owl:imports <{}> ; owl:imports <{}> .\n",
            ont_a, ont_b, ont_c
        ),
    )?;

    // B imports C
    add_bytes(
        &mut env,
        "urn:test:B",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology ; owl:imports <{}> .\n",
            ont_b, ont_c
        ),
    )?;

    // C is standalone
    add_bytes(
        &mut env,
        "urn:test:C",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology .\n",
            ont_c
        ),
    )?;

    // Add alias for B
    env.add_alias(ont_b_alias, ont_b)?;

    // Compute closure from A
    let a_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(ont_a)?))
        .expect("A should be in env");
    let closure = env.get_closure(&a_id, -1)?;

    // Expected: A, B, C (3 unique ontologies)
    // Even though B is imported directly by A and B has an alias,
    // B should only appear once
    assert_eq!(closure.len(), 3, "closure should be [A, B, C]");

    // Verify B appears exactly once in the closure
    let b_count = closure
        .iter()
        .filter(|id| id.to_uri_string() == ont_b)
        .count();
    assert_eq!(b_count, 1, "B should appear exactly once in closure");

    Ok(())
}

/// Test alias deduplication when the same ontology is imported via alias and canonical.
#[test]
fn alias_import_via_both_paths_deduplicates() -> Result<()> {
    let mut env = in_memory_env()?;

    let ont_a = "http://example.com/A";
    let ont_b = "http://example.com/B";
    let ont_b_alias = "http://example.com/B-alias";

    // A imports B via canonical IRI and also via alias
    add_bytes(
        &mut env,
        "urn:test:A",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology ; owl:imports <{}> ; owl:imports <{}> .\n",
            ont_a, ont_b, ont_b_alias
        ),
    )?;

    // B is standalone
    add_bytes(
        &mut env,
        "urn:test:B",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology .\n",
            ont_b
        ),
    )?;

    // Add alias for B
    env.add_alias(ont_b_alias, ont_b)?;

    // Compute closure from A
    let a_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(ont_a)?))
        .expect("A should be in env");
    let closure = env.get_closure(&a_id, -1)?;

    // Expected: A, B (2 unique ontologies)
    // Even though B is imported twice (once via canonical, once via alias),
    // B should only appear once
    assert_eq!(closure.len(), 2, "closure should be [A, B]");

    // Verify B appears exactly once
    let b_count = closure
        .iter()
        .filter(|id| id.to_uri_string() == ont_b)
        .count();
    assert_eq!(b_count, 1, "B should appear exactly once in closure");

    Ok(())
}

/// Test that aliases work correctly with circular imports.
#[test]
fn alias_with_circular_imports() -> Result<()> {
    let mut env = in_memory_env()?;

    let ont_a = "http://example.com/A";
    let ont_a_alias = "http://example.com/A-alias";
    let ont_b = "http://example.com/B";

    // A imports B, B imports A (circular)
    add_bytes(
        &mut env,
        "urn:test:A",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology ; owl:imports <{}> .\n",
            ont_a, ont_b
        ),
    )?;

    add_bytes(
        &mut env,
        "urn:test:B",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology ; owl:imports <{}> .\n",
            ont_b, ont_a
        ),
    )?;

    // Add alias for A
    env.add_alias(ont_a_alias, ont_a)?;

    // Compute closure from A
    let a_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(ont_a)?))
        .expect("A should be in env");
    let closure = env.get_closure(&a_id, -1)?;

    // Expected: A, B (2 unique ontologies, circular but no duplicates)
    assert_eq!(closure.len(), 2, "closure should be [A, B]");

    // Verify no duplicates
    let a_count = closure
        .iter()
        .filter(|id| id.to_uri_string() == ont_a)
        .count();
    let b_count = closure
        .iter()
        .filter(|id| id.to_uri_string() == ont_b)
        .count();
    assert_eq!(a_count, 1, "A should appear exactly once");
    assert_eq!(b_count, 1, "B should appear exactly once");

    // Compute closure from alias - should give same result
    let alias_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(ont_a_alias)?))
        .expect("alias should be in env");
    let alias_closure = env.get_closure(&alias_id, -1)?;

    assert_eq!(
        alias_closure.len(),
        2,
        "closure from alias should be [A, B]"
    );

    Ok(())
}

/// Test that aliases are properly excluded when computing closure with depth limit.
#[test]
fn alias_with_recursion_depth_limit() -> Result<()> {
    let mut env = in_memory_env()?;

    let ont_a = "http://example.com/A";
    let ont_a_alias = "http://example.com/A-alias";
    let ont_b = "http://example.com/B";
    let ont_c = "http://example.com/C";

    // A imports B, B imports C
    add_bytes(
        &mut env,
        "urn:test:A",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology ; owl:imports <{}> .\n",
            ont_a, ont_b
        ),
    )?;

    add_bytes(
        &mut env,
        "urn:test:B",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology ; owl:imports <{}> .\n",
            ont_b, ont_c
        ),
    )?;

    // C is standalone
    add_bytes(
        &mut env,
        "urn:test:C",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology .\n",
            ont_c
        ),
    )?;

    // Add alias for A
    env.add_alias(ont_a_alias, ont_a)?;

    // Compute closure with depth 0 (only root)
    let a_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(ont_a)?))
        .expect("A should be in env");
    let closure_depth_0 = env.get_closure(&a_id, 0)?;
    assert_eq!(closure_depth_0.len(), 1, "depth 0 should only include A");

    // Compute closure with depth 1 (root + direct imports)
    let closure_depth_1 = env.get_closure(&a_id, 1)?;
    assert_eq!(closure_depth_1.len(), 2, "depth 1 should include A and B");

    // Compute closure from alias with depth 1
    let alias_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(ont_a_alias)?))
        .expect("alias should be in env");
    let alias_closure_depth_1 = env.get_closure(&alias_id, 1)?;

    assert_eq!(
        alias_closure_depth_1.len(),
        2,
        "alias depth 1 should include A and B"
    );

    Ok(())
}

/// Test that aliases don't create duplicate references when the same ontology
/// is imported multiple times via different aliases pointing to the same target.
#[test]
fn multiple_aliases_to_same_target_deduplicates() -> Result<()> {
    let mut env = in_memory_env()?;

    let ont_a = "http://example.com/A";
    let ont_b = "http://example.com/B";
    let ont_b_alias1 = "http://example.com/B-alias1";
    let ont_b_alias2 = "http://example.com/B-alias2";

    // A imports B multiple times via different aliases
    add_bytes(
        &mut env,
        "urn:test:A",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology ; owl:imports <{}> ; owl:imports <{}> ; owl:imports <{}> .\n",
            ont_a, ont_b, ont_b_alias1, ont_b_alias2
        ),
    )?;

    // B is standalone
    add_bytes(
        &mut env,
        "urn:test:B",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology .\n",
            ont_b
        ),
    )?;

    // Add multiple aliases for B
    env.add_alias(ont_b_alias1, ont_b)?;
    env.add_alias(ont_b_alias2, ont_b)?;

    // Compute closure from A
    let a_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(ont_a)?))
        .expect("A should be in env");
    let closure = env.get_closure(&a_id, -1)?;

    // Expected: A, B (2 unique ontologies)
    // Even though B is imported 3 times (direct + 2 aliases),
    // B should only appear once
    assert_eq!(closure.len(), 2, "closure should be [A, B]");

    // Verify B appears exactly once
    let b_count = closure
        .iter()
        .filter(|id| id.to_uri_string() == ont_b)
        .count();
    assert_eq!(b_count, 1, "B should appear exactly once in closure");

    Ok(())
}

/// Test that blank nodes in the ontology graph don't cause issues with
/// alias deduplication. The key is that blank nodes can't be compared for
/// equality, so if the code tries to use them as keys in deduplication,
/// it would fail or create duplicates.
#[test]
fn multiple_aliases_to_same_target_deduplicates_with_blank_nodes() -> Result<()> {
    let mut env = in_memory_env()?;

    let ont_a = "http://example.com/A";
    let ont_b = "http://example.com/B";
    let ont_b_alias1 = "http://example.com/B-alias1";
    let ont_b_alias2 = "http://example.com/B-alias2";

    // A imports B multiple times via different aliases
    add_bytes(
        &mut env,
        "urn:test:A",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{}> a owl:Ontology ;\n\
             owl:imports <{}> ;\n\
             owl:imports <{}> ;\n\
             owl:imports <{}> .\n",
            ont_a, ont_b, ont_b_alias1, ont_b_alias2
        ),
    )?;

    // B contains blank nodes that can't be compared
    add_bytes(
        &mut env,
        "urn:test:B",
        &format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
            <{}> a owl:Ontology .\n\
            _:b0 <http://example.com/prop> \"value\" .\n\
            _:b1 <http://example.com/other> \"data\" .\n",
            ont_b
        ),
    )?;

    // Add multiple aliases for B
    env.add_alias(ont_b_alias1, ont_b)?;
    env.add_alias(ont_b_alias2, ont_b)?;

    // Compute closure from A
    let a_id = env
        .resolve(ResolveTarget::Graph(NamedNode::new(ont_a)?))
        .expect("A should be in env");
    let closure = env.get_closure(&a_id, -1)?;

    // We should have exactly 2 ontologies: A and B
    // Even though B is imported 3 times (direct + 2 aliases),
    // blank nodes in B's graph shouldn't cause issues with deduplication
    assert_eq!(closure.len(), 2, "closure should be [A, B]");

    // Verify B appears exactly once
    let b_count = closure
        .iter()
        .filter(|id| id.to_uri_string() == ont_b)
        .count();
    assert_eq!(b_count, 1, "B should appear exactly once in closure");

    // Verify the total number of triples
    // A has: 1 ontology declaration + 3 owl:imports = 4 triples
    // B has: 1 ontology declaration + 2 triples with blank nodes = 3 triples
    // Total should be 7 triples
    let total_triples: usize = closure
        .iter()
        .map(|id| env.get_graph(id).unwrap().len())
        .sum();
    assert_eq!(
        total_triples, 7,
        "total triples should be 7 (4 from A + 3 from B)"
    );

    Ok(())
}

// ── Config persistence tests ──────────────────────────────────────────────────

/// Scalar flags written during `init` must survive a full save→reload cycle.
#[test]
fn config_flags_persist_across_reload() -> Result<()> {
    let dir = new_tempdir("ontoenv-cfg-persist")?;
    let cfg = Config::builder()
        .root(dir.path().into())
        .locations(vec![dir.path().into()])
        .offline(true)
        .strict(true)
        .require_ontology_names(true)
        .remote_cache_ttl_secs(3600)
        .build()?;

    let env = OntoEnv::init(cfg, false)?;
    drop(env);

    let reloaded = OntoEnv::load_from_directory(dir.path().into(), false)?;
    assert!(reloaded.is_offline(), "offline should survive reload");
    assert!(reloaded.is_strict(), "strict should survive reload");
    assert!(
        reloaded.requires_ontology_names(),
        "require_ontology_names should survive reload"
    );

    teardown(dir);
    Ok(())
}

/// Reopening preserves persisted configuration unless an override is explicit.
#[test]
fn open_or_init_uses_explicit_overrides_on_existing_env() -> Result<()> {
    let dir = new_tempdir("ontoenv-open-or-init-flags")?;

    // Create an env with offline=false (the default).
    let online_cfg = Config::builder()
        .root(dir.path().into())
        .locations(vec![dir.path().into()])
        .offline(false)
        .build()?;
    let env = OntoEnv::init(online_cfg, false)?;
    assert!(!env.is_offline());
    drop(env);

    // Values on the initialization config are not treated as reopen
    // overrides, because Config itself cannot distinguish defaults.
    let offline_cfg = Config::builder()
        .root(dir.path().into())
        .offline(true)
        .build()?;
    let env2 = OntoEnv::open_or_init(offline_cfg.clone(), false)?;
    assert!(!env2.is_offline(), "omitted override should preserve false");
    drop(env2);

    let overrides = ConfigOverrides {
        offline: Some(true),
        strict: Some(true),
        require_ontology_names: Some(true),
        remote_cache_ttl_secs: Some(17),
        use_cached_ontologies: Some(CacheMode::Enabled),
        resolution_policy: Some("latest".to_string()),
        locations: Some(Vec::new()),
        includes: Some(Vec::new()),
        excludes: Some(vec!["ignored/**".to_string()]),
        include_ontologies: Some(vec!["example".to_string()]),
        exclude_ontologies: Some(Vec::new()),
    };
    let env2 = OntoEnv::open_or_init_with_overrides(offline_cfg, false, overrides)?;
    assert!(env2.is_offline());
    assert!(env2.is_strict());
    assert!(env2.requires_ontology_names());
    assert_eq!(env2.remote_cache_ttl_secs(), 17);
    assert!(env2.uses_cached_ontologies());
    assert_eq!(env2.resolution_policy(), "latest");
    drop(env2);

    // Explicit overrides persist, while a subsequent plain load is inert.
    let env3 = OntoEnv::load_from_directory(dir.path().into(), false)?;
    assert!(env3.is_offline());
    assert!(env3.is_strict());
    assert!(env3.requires_ontology_names());
    assert_eq!(env3.remote_cache_ttl_secs(), 17);
    assert!(env3.uses_cached_ontologies());
    assert_eq!(env3.resolution_policy(), "latest");

    teardown(dir);
    Ok(())
}

/// The mechanism used by `new_offline`: loading an online env, switching the
/// offline flag, and saving must persist so the next load honours it.
#[test]
fn new_offline_mechanism_persists_flag() -> Result<()> {
    let dir = new_tempdir("ontoenv-new-offline")?;

    // Start online.
    let online_cfg = Config::builder()
        .root(dir.path().into())
        .offline(false)
        .build()?;
    let env = OntoEnv::init(online_cfg, false)?;
    assert!(!env.is_offline());
    drop(env);

    // Simulate what new_offline() does when it finds an existing env.
    let mut loaded = OntoEnv::load_from_directory(dir.path().into(), false)?;
    assert!(!loaded.is_offline());
    if !loaded.is_offline() {
        loaded.set_offline(true);
        loaded.save_to_directory()?;
    }
    assert!(loaded.is_offline());
    drop(loaded);

    // The flag must survive a fresh load.
    let reloaded = OntoEnv::load_from_directory(dir.path().into(), false)?;
    assert!(reloaded.is_offline(), "offline=true must be persisted");

    teardown(dir);
    Ok(())
}

/// The mechanism used by `new_online`: loading an offline env, clearing the
/// offline flag, and saving must persist so the next load honours it.
#[test]
fn new_online_mechanism_persists_flag() -> Result<()> {
    let dir = new_tempdir("ontoenv-new-online")?;

    // Start offline.
    let offline_cfg = Config::builder()
        .root(dir.path().into())
        .offline(true)
        .build()?;
    let env = OntoEnv::init(offline_cfg, false)?;
    assert!(env.is_offline());
    drop(env);

    // Simulate what new_online() does when it finds an existing env.
    let mut loaded = OntoEnv::load_from_directory(dir.path().into(), false)?;
    assert!(loaded.is_offline());
    if loaded.is_offline() {
        loaded.set_offline(false);
        loaded.save_to_directory()?;
    }
    assert!(!loaded.is_offline());
    drop(loaded);

    let reloaded = OntoEnv::load_from_directory(dir.path().into(), false)?;
    assert!(!reloaded.is_offline(), "offline=false must be persisted");

    teardown(dir);
    Ok(())
}
