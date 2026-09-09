use ontoenv::ontology::{GraphIdentifier, OntologyLocation};
use ontoenv::FailedImport;
use oxigraph::model::NamedNode;
use url::Url;

#[test]
fn test_ontology_location() {
    let url = "http://example.com/ontology.ttl";
    let mut file_path = std::env::temp_dir();
    file_path.push("ontology.ttl");
    let file = file_path.to_string_lossy().to_string();
    let url_location = OntologyLocation::from_str(url).unwrap();
    let file_location = OntologyLocation::from_str(&file).unwrap();
    assert!(url_location.is_url());
    assert!(!url_location.is_file());
    assert!(!file_location.is_url());
    assert!(file_location.is_file());
}

#[test]
fn test_ontology_location_display() {
    // 1. Create a platform-agnostic path
    let mut path = std::env::temp_dir();
    path.push("ontology.ttl");

    // 2. Create the location
    let location = OntologyLocation::File(path.clone());

    // 3. Create the EXPECTED string correctly
    let expected_url_string = Url::from_file_path(&path).unwrap().to_string(); // Generates "file:///D:/tmp/ontology.ttl"

    // 4. The assertion will now pass
    // Note: Your Display impl might be "file://" (2 slashes). If so,
    // this assertion might still fail, revealing a small bug in your
    // Display implementation. But the test's expected value will be correct.
    assert_eq!(location.to_string(), expected_url_string);
}

#[test]
fn test_ontology_location_to_iri() {
    // 1. Create a platform-agnostic path
    let mut path = std::env::temp_dir(); // Gets D:\tmp on Windows, /tmp on Linux
    path.push("ontology.ttl"); // path is now "D:\tmp\ontology.ttl"

    // 2. Create the location from this path
    let location = OntologyLocation::File(path.clone());

    // 3. Create the EXPECTED IRI correctly
    let expected_url_string = Url::from_file_path(&path).unwrap().to_string(); // Generates "file:///D:/tmp/ontology.ttl"
    let expected_iri = NamedNode::new(expected_url_string).unwrap();

    // 4. The assertion will now pass on all platforms
    assert_eq!(location.to_iri(), expected_iri); // <-- REMOVED .unwrap()
}

#[test]
fn failed_import_exposes_structured_context() {
    let id = GraphIdentifier::new(
        oxigraph::model::NamedNodeRef::new("https://example.org/missing").unwrap(),
    );
    let failure = FailedImport::new(id.clone(), "backend unavailable".to_string());

    assert_eq!(failure.ontology(), &id);
    assert_eq!(failure.error(), "backend unavailable");
}

/// The parallel import path extracts ontology metadata from a parsed triple
/// list; the store-backed path is still used when adopting an existing
/// backend. Both must describe a real ontology identically.
#[test]
fn from_triples_matches_from_store() {
    use ontoenv::ontology::Ontology;
    use oxigraph::io::{RdfFormat, RdfParser};
    use oxigraph::model::{GraphNameRef, Quad, Triple};
    use oxigraph::store::Store;
    use std::path::PathBuf;

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../brick/Brick.ttl");
    let bytes = std::fs::read(&path).expect("read Brick.ttl fixture");
    let location = OntologyLocation::File(path);

    let triples: Vec<Triple> = RdfParser::from_format(RdfFormat::Turtle)
        .for_slice(&bytes)
        .map(|quad| quad.map(|q| Triple::new(q.subject, q.predicate, q.object)))
        .collect::<Result<_, _>>()
        .expect("parse Brick.ttl");
    assert!(triples.len() > 1000, "fixture should be a real ontology");

    let from_triples = Ontology::from_triples(&triples, location.clone(), true).unwrap();

    let staging = NamedNode::new_unchecked("temp:graph");
    let store = Store::new().unwrap();
    let mut loader = store.bulk_loader();
    loader
        .load_quads(triples.iter().map(|t| {
            Quad::new(
                t.subject.clone(),
                t.predicate.clone(),
                t.object.clone(),
                GraphNameRef::NamedNode(staging.as_ref()),
            )
        }))
        .unwrap();
    loader.commit().unwrap();
    let staging_id = GraphIdentifier::new_with_location(staging.as_ref(), location.clone());
    let from_store = Ontology::from_store(&store, &staging_id, true).unwrap();

    assert_eq!(from_triples.name(), from_store.name());
    assert_eq!(from_triples.id(), from_store.id());
    assert_eq!(from_triples.location(), from_store.location());
    let mut a = from_triples.imports.clone();
    let mut b = from_store.imports.clone();
    a.sort();
    b.sort();
    assert_eq!(a, b, "imports differ");
    assert!(!a.is_empty(), "Brick declares imports");
    assert_eq!(
        from_triples.namespace_map(),
        from_store.namespace_map(),
        "sh:declare prefixes differ"
    );
    assert!(!from_triples.namespace_map().is_empty());
    assert_eq!(
        from_triples.version_properties(),
        from_store.version_properties(),
        "version properties differ"
    );
    assert!(!from_triples.version_properties().is_empty());
}
