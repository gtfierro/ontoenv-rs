use std::fs;
use std::path::PathBuf;
use std::process::Command;

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

#[test]
fn why_lists_importers() {
    // temp dir
    let mut root = std::env::current_dir().expect("cwd");
    root.push(format!("target/test_cli_why_{}", std::process::id()));
    if root.exists() { let _ = fs::remove_dir_all(&root); }
    fs::create_dir_all(&root).expect("mkdir");

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

    // Locate built binary (debug or release)
    let mut bin_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("target").join("debug").join(if cfg!(windows) { "ontoenv.exe" } else { "ontoenv" });
    if !bin_path.exists() {
        bin_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("target").join("release").join(if cfg!(windows) { "ontoenv.exe" } else { "ontoenv" });
    }
    assert!(bin_path.exists(), "ontoenv binary not found at {:?}", bin_path);
    let exe = bin_path.to_string_lossy().to_string();

    // init with offline & include root as search dir
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("init")
        .arg("--overwrite")
        .arg("--offline")
        .output()
        .expect("run init");
    assert!(out.status.success(), "init failed: {}", String::from_utf8_lossy(&out.stderr));

    // run why for B; expect A -> B and C -> A -> B
    let out = Command::new(&exe)
        .current_dir(&root)
        .arg("why")
        .arg(b_uri)
        .output()
        .expect("run why");
    assert!(out.status.success(), "why failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Why"), "missing header: {stdout}");
    assert!(stdout.contains(&format!("{} -> {}", a_uri, b_uri)), "did not show A->B: {stdout}");
    assert!(
        stdout.contains(&format!("{} -> {} -> {}", c_uri, a_uri, b_uri)),
        "did not show C->A->B: {stdout}"
    );

    // clean up
    let _ = fs::remove_dir_all(&root);
}
