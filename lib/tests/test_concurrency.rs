use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use oxigraph::model::NamedNode;

use ontoenv::api::{OntoEnv, ResolveTarget};
use ontoenv::ontology::OntologyLocation;
use ontoenv::options::{Overwrite, RefreshStrategy};
use ontoenv::ToUriString;

/// Helper to write a small ontology TTL file.
fn write_ttl(path: &Path, ontology_uri: &str, extra: &str) {
    let content = format!(
        "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
         @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         <{uri}> a owl:Ontology .\n\
         {extra}\n",
        uri = ontology_uri,
        extra = extra
    );
    fs::write(path, content).expect("failed to write TTL file");
}

/// Create a fresh test root dir.
fn fresh_dir(name: &str) -> PathBuf {
    let mut dir = env::current_dir().expect("cwd");
    dir.push(format!("target/{}_{}", name, std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).ok();
    }
    fs::create_dir_all(&dir).expect("create test dir");
    dir
}

/// Initialize a persistent OntoEnv at root and add two ontologies.
fn init_store_with_two_graphs(root: &Path, a_uri: &str, b_uri: &str) -> (String, String) {
    // Build config and init env
    let config = ontoenv::config::Config::builder()
        .root(root.to_path_buf())
        .require_ontology_names(false)
        .strict(false)
        .offline(true)
        .temporary(false)
        .no_search(true)
        .build()
        .expect("build config");

    let mut env = OntoEnv::init(config, true).expect("init ontoenv");

    // Write two TTL files
    let a_path = root.join("A.ttl");
    let b_path = root.join("B.ttl");
    write_ttl(&a_path, a_uri, &format!("<{}#Class1> a owl:Class .", a_uri));
    write_ttl(&b_path, b_uri, &format!("<{}#Class2> a owl:Class .", b_uri));

    // Add to env without fetching imports
    let name_a = env
        .add(
            OntologyLocation::from_str(a_path.to_str().unwrap()).expect("loc a"),
            Overwrite::Preserve,
            RefreshStrategy::UseCache,
        )
        .expect("add A");
    let name_b = env
        .add(
            OntologyLocation::from_str(b_path.to_str().unwrap()).expect("loc b"),
            Overwrite::Preserve,
            RefreshStrategy::UseCache,
        )
        .expect("add B");

    env.flush().expect("flush");
    // Drop to release exclusive lock
    drop(env);

    (name_a.to_uri_string(), name_b.to_uri_string())
}

/// Returns the current test binary path.
fn current_test_exe() -> PathBuf {
    env::current_exe().expect("current_exe")
}

/// Worker: read-only open and fetch graph + metadata.
/// This is invoked as an ignored test inside this binary via libtest filter.
#[test]
#[ignore]
fn worker_ro() {
    let store = env::var("ONTOENV_STORE").expect("ONTOENV_STORE missing");
    let uri = env::var("ONTOENV_URI").expect("ONTOENV_URI missing");
    let root = PathBuf::from(store);

    // Load as read-only
    let env = OntoEnv::load_from_directory(root, true).expect("load read-only");
    let iri = NamedNode::new(&uri).expect("iri");
    let id = env
        .resolve(ResolveTarget::Graph(iri.clone()))
        .expect("resolve id");
    let g = env.get_graph(&id).expect("get_graph");
    assert!(g.len() > 0, "graph should have triples");
    let ont = env.get_ontology(&id).expect("get_ontology");
    assert_eq!(ont.id().name().as_str(), iri.as_str());

    // Signal to parent by stdout
    println!("worker_ro ok {}", iri);
}

/// Worker: read-write open; if lock acquired, fetch graph + metadata; otherwise verify lock error.
/// This is invoked as an ignored test inside this binary via libtest filter.
#[test]
#[ignore]
fn worker_rw() {
    let store = env::var("ONTOENV_STORE").expect("ONTOENV_STORE missing");
    let uri = env::var("ONTOENV_URI").expect("ONTOENV_URI missing");
    let root = PathBuf::from(store);

    match OntoEnv::load_from_directory(root, false) {
        Ok(env) => {
            // Acquired lock; do a read
            let iri = NamedNode::new(&uri).expect("iri");
            let id = env
                .resolve(ResolveTarget::Graph(iri.clone()))
                .expect("resolve id");
            let g = env.get_graph(&id).expect("get_graph");
            assert!(g.len() > 0, "graph should have triples");
            let ont = env.get_ontology(&id).expect("get_ontology");
            assert_eq!(ont.id().name().as_str(), iri.as_str());
            println!("worker_rw acquired {}", iri);
        }
        Err(e) => {
            // Should be lock acquisition failure
            let msg = format!("{e}");
            assert!(
                msg.contains("Failed to open OntoEnv store for write")
                    || msg.contains("exclusive lock"),
                "unexpected error: {msg}"
            );
            println!("worker_rw lockerror {}", uri);
        }
    }
}

/// Test: two read-only processes can open concurrently and fetch different graphs.
#[test]
fn rust_read_only_concurrency() {
    let root = fresh_dir("ontoenv_ro_test");
    let a_uri = "http://example.org/ont/A";
    let b_uri = "http://example.org/ont/B";
    let (name_a, name_b) = init_store_with_two_graphs(&root, a_uri, b_uri);

    let exe = current_test_exe();
    let p1 = Command::new(&exe)
        .arg("--exact")
        .arg("worker_ro")
        .arg("--ignored")
        .arg("--nocapture")
        .env("ONTOENV_STORE", root.to_string_lossy().to_string())
        .env("ONTOENV_URI", name_a.clone())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn p1");
    let p2 = Command::new(&exe)
        .arg("--exact")
        .arg("worker_ro")
        .arg("--ignored")
        .arg("--nocapture")
        .env("ONTOENV_STORE", root.to_string_lossy().to_string())
        .env("ONTOENV_URI", name_b.clone())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn p2");

    let o1 = p1.wait_with_output().expect("p1 output");
    let o2 = p2.wait_with_output().expect("p2 output");

    assert!(o1.status.success(), "p1 failed: {:?}", o1);
    assert!(o2.status.success(), "p2 failed: {:?}", o2);

    let s1 = String::from_utf8_lossy(&o1.stdout);
    let s2 = String::from_utf8_lossy(&o2.stdout);
    assert!(s1.contains("worker_ro ok"), "unexpected p1 stdout: {}", s1);
    assert!(s2.contains("worker_ro ok"), "unexpected p2 stdout: {}", s2);

    // cleanup
    fs::remove_dir_all(&root).ok();
}

/// Test: two read-write processes contend; one acquires lock, the other reports lock error.
#[test]
fn rust_read_write_locking() {
    let root = fresh_dir("ontoenv_rw_test");
    let a_uri = "http://example.org/ont/A";
    let b_uri = "http://example.org/ont/B";
    let (name_a, name_b) = init_store_with_two_graphs(&root, a_uri, b_uri);

    let exe = current_test_exe();
    let p1 = Command::new(&exe)
        .arg("--exact")
        .arg("worker_rw")
        .arg("--ignored")
        .arg("--nocapture")
        .env("ONTOENV_STORE", root.to_string_lossy().to_string())
        .env("ONTOENV_URI", name_a.clone())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn p1");
    let p2 = Command::new(&exe)
        .arg("--exact")
        .arg("worker_rw")
        .arg("--ignored")
        .arg("--nocapture")
        .env("ONTOENV_STORE", root.to_string_lossy().to_string())
        .env("ONTOENV_URI", name_b.clone())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn p2");

    let o1 = p1.wait_with_output().expect("p1 output");
    let o2 = p2.wait_with_output().expect("p2 output");

    assert!(o1.status.success(), "p1 failed: {:?}", o1);
    assert!(o2.status.success(), "p2 failed: {:?}", o2);

    let s1 = String::from_utf8_lossy(&o1.stdout);
    let s2 = String::from_utf8_lossy(&o2.stdout);

    // Ensure we saw one acquire and one lock error (order not guaranteed)
    let acquired =
        s1.contains("worker_rw acquired") as usize + s2.contains("worker_rw acquired") as usize;
    let lockerror =
        s1.contains("worker_rw lockerror") as usize + s2.contains("worker_rw lockerror") as usize;

    assert!(
        acquired >= 1,
        "expected at least one acquisition; stdout1: {}, stdout2: {}",
        s1,
        s2
    );
    assert!(
        lockerror >= 1,
        "expected at least one lock error; stdout1: {}, stdout2: {}",
        s1,
        s2
    );

    // cleanup
    fs::remove_dir_all(&root).ok();
}
