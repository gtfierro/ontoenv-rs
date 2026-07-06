use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn ontoenv_bin() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("debug")
        .join(if cfg!(windows) {
            "ontoenv.exe"
        } else {
            "ontoenv"
        });
    if !p.exists() {
        p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("release")
            .join(if cfg!(windows) {
                "ontoenv.exe"
            } else {
                "ontoenv"
            });
    }
    assert!(p.exists(), "ontoenv binary not found at {:?}", p);
    p
}

fn tmp_dir(name: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!("ontoenv-cli-{}-{}", name, std::process::id()));
    if base.exists() {
        let _ = fs::remove_dir_all(&base);
    }
    fs::create_dir_all(&base).unwrap();
    base
}

fn write_ttl(path: &PathBuf, ontology_uri: &str, extra: &str) {
    let content = format!(
        "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
         @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         <{uri}> a owl:Ontology .\n\
         {extra}\n",
        uri = ontology_uri,
        extra = extra
    );
    fs::write(path, content).expect("write ttl");
}

// Git-like semantics
#[test]
fn non_init_command_errors_outside_env() {
    let exe = ontoenv_bin();
    let root = tmp_dir("noenv");
    let out = Command::new(&exe)
        .current_dir(&root)
        .env("ONTOENV_DIR", &root)
        .arg("list")
        .arg("ontologies")
        .output()
        .expect("run list");
    assert!(!out.status.success(), "expected failure outside env");
}

#[test]
fn discovery_from_subdirectory() {
    let exe = ontoenv_bin();
    let root = tmp_dir("discover");
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("init")
        .arg(".")
        .output()
        .expect("run init");
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Initialized environment with"),
        "init summary missing: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let nested = root.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let out = Command::new(&exe)
        .current_dir(&nested)
        .arg("list")
        .arg("ontologies")
        .output()
        .expect("run list");
    assert!(
        out.status.success(),
        "list failed in subdir: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn ontoenv_dir_override() {
    let exe = ontoenv_bin();
    let env_root = tmp_dir("envdir");
    let out = Command::new(&exe)
        .current_dir(&env_root)
        .arg("init")
        .arg(".")
        .output()
        .expect("run init");
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Initialized environment with"),
        "init summary missing: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let elsewhere = tmp_dir("elsewhere");
    let out = Command::new(&exe)
        .current_dir(&elsewhere)
        .env("ONTOENV_DIR", env_root.join(".ontoenv"))
        .arg("list")
        .arg("ontologies")
        .output()
        .expect("run list");
    assert!(
        out.status.success(),
        "list failed with ONTOENV_DIR: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn update_from_nested_subdir_uses_root_locations() {
    let exe = ontoenv_bin();
    let root = tmp_dir("update_nested");
    let ont_dir = root.join("ontologies");
    fs::create_dir_all(&ont_dir).unwrap();
    let ont_path = ont_dir.join("A.ttl");
    write_ttl(&ont_path, "http://example.org/ont/A", "");

    // Ensure file mtime changes on rewrites (Linux FS timestamp granularity can be 1s)
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("init")
        .arg("ontologies")
        .output()
        .expect("run init");
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Initialized environment with"),
        "init summary missing: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Modify the file to ensure update detects a change.
    write_ttl(
        &ont_path,
        "http://example.org/ont/A",
        "<http://example.org/ont/A> <http://example.org/p> <http://example.org/o> .",
    );
    // Ensure mtime advances on filesystems with coarse timestamp granularity (e.g., Windows).
    std::thread::sleep(std::time::Duration::from_millis(2000));

    let nested = root.join("nested").join("deeper");
    fs::create_dir_all(&nested).unwrap();
    let out = Command::new(&exe)
        .current_dir(&nested)
        .arg("update")
        .output()
        .expect("run update");
    assert!(
        out.status.success(),
        "update failed from nested dir: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let update_stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        update_stdout.contains("Updated ") && update_stdout.contains("ontolog"),
        "update summary output unexpected: {}",
        update_stdout
    );

    write_ttl(
        &ont_path,
        "http://example.org/ont/A",
        "<http://example.org/ont/A> <http://example.org/p2> <http://example.org/o2> .",
    );
    std::thread::sleep(std::time::Duration::from_millis(2000));

    let out = Command::new(&exe)
        .current_dir(&nested)
        .arg("--verbose")
        .arg("update")
        .output()
        .expect("run verbose update");
    assert!(
        out.status.success(),
        "verbose update failed from nested dir: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let verbose_stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        verbose_stdout.contains("Updated ")
            && verbose_stdout.contains("ontolog")
            && verbose_stdout.contains("http://example.org/ont/A"),
        "verbose update output missing expected detail: {}",
        verbose_stdout
    );

    let out = Command::new(&exe)
        .current_dir(&nested)
        .arg("list")
        .arg("locations")
        .output()
        .expect("run list locations");
    assert!(
        out.status.success(),
        "list locations failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let locations_out = String::from_utf8_lossy(&out.stdout);
    assert!(
        locations_out.contains("ontologies") && locations_out.contains("A.ttl"),
        "expected ontology location in output, got: {}",
        locations_out
    );
}

// Why subcommand integration
#[test]
fn why_lists_importers_paths() {
    let exe = ontoenv_bin();
    let root = tmp_dir("why");
    // three ontologies: C imports A; A imports B
    let a_uri = "http://example.org/ont/A";
    let b_uri = "http://example.org/ont/B";
    let c_uri = "http://example.org/ont/C";
    let a_path = root.join("A.ttl");
    let b_path = root.join("B.ttl");
    let c_path = root.join("C.ttl");
    write_ttl(&b_path, b_uri, "");
    write_ttl(
        &a_path,
        a_uri,
        &format!("<{}> owl:imports <{}> .", a_uri, b_uri),
    );
    write_ttl(
        &c_path,
        c_uri,
        &format!("<{}> owl:imports <{}> .", c_uri, a_uri),
    );

    // init
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("init")
        .arg(".")
        .output()
        .expect("run init");
    assert!(out.status.success());

    // why B should show A->B and C->A->B
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("why")
        .arg(b_uri)
        .output()
        .expect("run why");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("{} -> {}", a_uri, b_uri)));
    assert!(stdout.contains(&format!("{} -> {} -> {}", c_uri, a_uri, b_uri)));
}

// Get command: default Turtle to STDOUT by IRI
#[test]
fn get_stdout_turtle() {
    let exe = ontoenv_bin();
    let root = tmp_dir("get_turtle");
    let iri = "http://example.org/ont/Only";
    let path = root.join("only.ttl");
    write_ttl(&path, iri, "");

    // init
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("init")
        .arg(".")
        .output()
        .expect("run init");
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // get to stdout
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("get")
        .arg(iri)
        .output()
        .expect("run get");
    assert!(
        out.status.success(),
        "get failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Expect to see the ontology triple in some form
    assert!(
        stdout.contains(iri),
        "stdout did not contain IRI: {}",
        stdout
    );
}

// Get command: JSON-LD output
#[test]
fn get_jsonld_output() {
    let exe = ontoenv_bin();
    let root = tmp_dir("get_jsonld");
    let iri = "http://example.org/ont/JL";
    let path = root.join("jl.ttl");
    write_ttl(&path, iri, "");

    // init
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("init")
        .arg(".")
        .output()
        .expect("run init");
    assert!(out.status.success());

    // get jsonld to stdout
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("get")
        .arg(iri)
        .arg("--format")
        .arg("jsonld")
        .output()
        .expect("run get jsonld");
    assert!(
        out.status.success(),
        "get jsonld failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(iri),
        "jsonld output missing iri; got: {}",
        stdout
    );
    assert!(
        stdout.trim_start().starts_with("{") || stdout.trim_start().starts_with("["),
        "not JSON-LD? {}",
        stdout
    );
}

// Get command: disambiguate with --location when same IRI at two locations
#[test]
fn get_with_location_disambiguates() {
    let exe = ontoenv_bin();
    let root = tmp_dir("get_loc");
    let iri = "http://example.org/ont/Dup";
    let p1 = root.join("dup_v1.ttl");
    let p2 = root.join("dup_v2.ttl");
    // add distinguishing triples
    write_ttl(
        &p1,
        iri,
        "<http://example.org/x> <http://example.org/p> \"v1\" .",
    );
    write_ttl(
        &p2,
        iri,
        "<http://example.org/x> <http://example.org/p> \"v2\" .",
    );

    // init
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("init")
        .arg(".")
        .output()
        .expect("run init");
    assert!(out.status.success());

    // get with location pointing to v1
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("get")
        .arg(iri)
        .arg("--location")
        .arg(p1.to_str().unwrap())
        .output()
        .expect("run get v1");
    assert!(
        out.status.success(),
        "get v1 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s1 = String::from_utf8_lossy(&out.stdout);
    assert!(s1.contains("\"v1\""), "expected v1 triple, got: {}", s1);

    // get with location pointing to v2
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("get")
        .arg(iri)
        .arg("-l")
        .arg(p2.to_str().unwrap())
        .output()
        .expect("run get v2");
    assert!(
        out.status.success(),
        "get v2 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s2 = String::from_utf8_lossy(&out.stdout);
    assert!(s2.contains("\"v2\""), "expected v2 triple, got: {}", s2);
}

/// `ontoenv init --offline` on an already-initialized environment must persist
/// the offline flag to ontoenv.json so subsequent loads honour it.
#[test]
fn init_offline_flag_persists_on_existing_env() {
    let exe = ontoenv_bin();
    let root = tmp_dir("init-offline-persist");

    // First init without --offline.
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("init")
        .arg(".")
        .output()
        .expect("run init");
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg_path = root.join(".ontoenv").join("ontoenv.json");
    let raw = fs::read_to_string(&cfg_path).expect("read ontoenv.json");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("parse json");
    assert_eq!(
        json["offline"].as_bool(),
        Some(false),
        "offline should be false after first init"
    );

    // Second init with --offline on the existing env (no --overwrite).
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("--offline")
        .arg("init")
        .arg(".")
        .output()
        .expect("run init --offline");
    assert!(
        out.status.success(),
        "init --offline failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let raw2 = fs::read_to_string(&cfg_path).expect("read ontoenv.json after second init");
    let json2: serde_json::Value = serde_json::from_str(&raw2).expect("parse json2");
    assert_eq!(
        json2["offline"].as_bool(),
        Some(true),
        "offline must be true after `ontoenv init --offline` on existing env"
    );
}

/// `ontoenv union` takes multiple ontology IRIs and writes a merged file.
/// With --include-closures, the transitive imports of each listed IRI are
/// included in the union.
#[test]
fn union_command_merges_ontologies_with_closures() {
    let exe = ontoenv_bin();
    let root = tmp_dir("union-test");

    // Write three chained ontologies: C imports B, B imports A
    let a_path = root.join("A.ttl");
    write_ttl(
        &a_path,
        "http://example.com/A",
        "<http://example.com/A> <http://example.com/p> \"a-val\" .",
    );
    let b_path = root.join("B.ttl");
    write_ttl(
        &b_path,
        "http://example.com/B",
        "<http://example.com/B> <http://www.w3.org/2002/07/owl#imports> <http://example.com/A> .\n<http://example.com/B> <http://example.com/p> \"b-val\" .",
    );
    let c_path = root.join("C.ttl");
    write_ttl(
        &c_path,
        "http://example.com/C",
        "<http://example.com/C> <http://www.w3.org/2002/07/owl#imports> <http://example.com/B> .\n<http://example.com/C> <http://example.com/p> \"c-val\" .",
    );

    // Init the env
    let init_out = Command::new(&exe)
        .current_dir(&root)
        .arg("init")
        .arg("--offline")
        .arg(root.to_str().unwrap())
        .output()
        .expect("run init");
    assert!(
        init_out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    // Add each ontology explicitly so they're registered under the expected IRIs
    for path in [&a_path, &b_path, &c_path] {
        let add_out = Command::new(&exe)
            .current_dir(&root)
            .arg("add")
            .arg(path.to_str().unwrap())
            .output()
            .expect("run add");
        assert!(
            add_out.status.success(),
            "add failed for {:?}: {}",
            path,
            String::from_utf8_lossy(&add_out.stderr)
        );
    }

    // Union just C, with --include-closures, which should pull in B and A
    let output_path = root.join("union-output.ttl");
    let union_out = Command::new(&exe)
        .current_dir(&root)
        .arg("union")
        .arg("--root")
        .arg("http://example.com/C")
        .arg("--include-closures")
        .arg("--output")
        .arg(output_path.to_str().unwrap())
        .arg("http://example.com/C")
        .output()
        .expect("run union");
    assert!(
        union_out.status.success(),
        "union failed: {}",
        String::from_utf8_lossy(&union_out.stderr)
    );

    // The output file should exist
    assert!(output_path.exists(), "union output file should exist");
    let output_content = fs::read_to_string(&output_path).expect("read union output");

    // Should contain triples from all three ontologies
    assert!(
        output_content.contains("\"a-val\""),
        "output should contain A's triple"
    );
    assert!(
        output_content.contains("\"b-val\""),
        "output should contain B's triple"
    );
    assert!(
        output_content.contains("\"c-val\""),
        "output should contain C's triple"
    );

    // owl:imports triples should be removed by default
    assert!(
        !output_content.contains("owl:imports"),
        "owl:imports should be removed by default"
    );
}

/// Test the `ontoenv add --rename` flag
#[test]
fn add_with_rename_stores_under_new_iri() {
    let exe = ontoenv_bin();
    let root = tmp_dir("add-rename");

    let old_iri = "http://example.com/OldOnt";
    let new_iri = "http://example.com/NewOnt";
    let ttl_path = root.join("ont.ttl");
    write_ttl(&ttl_path, old_iri, "");

    // Init env with the file location
    let init_out = Command::new(&exe)
        .current_dir(&root)
        .arg("init")
        .arg(".")
        .output()
        .expect("init");
    assert!(init_out.status.success());

    // Add with rename
    let add_out = Command::new(&exe)
        .current_dir(&root)
        .arg("add")
        .arg("--rename")
        .arg(new_iri)
        .arg(ttl_path.to_str().unwrap())
        .output()
        .expect("add with rename");
    assert!(
        add_out.status.success(),
        "add --rename failed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    // Verify the new IRI is in the env and the old is gone
    let list_out = Command::new(&exe)
        .current_dir(&root)
        .arg("list")
        .arg("ontologies")
        .output()
        .expect("list");
    assert!(list_out.status.success());
    let stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        stdout.contains(new_iri),
        "new IRI should be listed: {stdout}"
    );
    assert!(
        !stdout.contains(old_iri),
        "old IRI should NOT be listed: {stdout}"
    );
}
