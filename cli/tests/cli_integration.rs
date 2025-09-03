use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn ontoenv_bin() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("debug")
        .join(if cfg!(windows) { "ontoenv.exe" } else { "ontoenv" });
    if !p.exists() {
        p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("release")
            .join(if cfg!(windows) { "ontoenv.exe" } else { "ontoenv" });
    }
    assert!(p.exists(), "ontoenv binary not found at {:?}", p);
    p
}

fn tmp_dir(name: &str) -> PathBuf {
    let mut d = std::env::current_dir().unwrap();
    d.push(format!("target/cli_integration_{}_{}", name, std::process::id()));
    if d.exists() { let _ = fs::remove_dir_all(&d); }
    fs::create_dir_all(&d).unwrap();
    d
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
        .output()
        .expect("run init");
    assert!(out.status.success(), "init failed: {}", String::from_utf8_lossy(&out.stderr));
    let nested = root.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let out = Command::new(&exe)
        .current_dir(&nested)
        .arg("list")
        .arg("ontologies")
        .output()
        .expect("run list");
    assert!(out.status.success(), "list failed in subdir: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn ontoenv_dir_override() {
    let exe = ontoenv_bin();
    let env_root = tmp_dir("envdir");
    let out = Command::new(&exe)
        .current_dir(&env_root)
        .arg("init")
        .output()
        .expect("run init");
    assert!(out.status.success(), "init failed: {}", String::from_utf8_lossy(&out.stderr));
    let elsewhere = tmp_dir("elsewhere");
    let out = Command::new(&exe)
        .current_dir(&elsewhere)
        .env("ONTOENV_DIR", env_root.join(".ontoenv"))
        .arg("list")
        .arg("ontologies")
        .output()
        .expect("run list");
    assert!(out.status.success(), "list failed with ONTOENV_DIR: {}", String::from_utf8_lossy(&out.stderr));
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
    write_ttl(&a_path, a_uri, &format!("<{}> owl:imports <{}> .", a_uri, b_uri));
    write_ttl(&c_path, c_uri, &format!("<{}> owl:imports <{}> .", c_uri, a_uri));

    // init
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("init")
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

