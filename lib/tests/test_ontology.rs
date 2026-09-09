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

#[test]
fn from_graph_extracts_brick_metadata() {
    use ontoenv::ontology::Ontology;
    use oxigraph::io::{RdfFormat, RdfParser};
    use oxigraph::model::Graph;
    use std::path::PathBuf;

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../brick/Brick.ttl");
    let bytes = std::fs::read(&path).expect("read Brick.ttl fixture");
    let mut graph = Graph::new();
    for quad in RdfParser::from_format(RdfFormat::Turtle).for_slice(&bytes) {
        let quad = quad.expect("parse Brick.ttl");
        graph.insert(&oxigraph::model::Triple::new(
            quad.subject,
            quad.predicate,
            quad.object,
        ));
    }

    let ontology = Ontology::from_graph(&graph, OntologyLocation::File(path), true).unwrap();
    assert_eq!(
        ontology.name().as_str(),
        "https://brickschema.org/schema/1.4/Brick"
    );
    assert!(ontology
        .imports
        .iter()
        .any(|i| i.as_str() == "http://data.ashrae.org/bacnet/2020"));
    assert_eq!(
        ontology.namespace_map().get("rec").map(String::as_str),
        Some("https://w3id.org/rec#")
    );
    assert_eq!(
        ontology
            .version_properties()
            .get(&NamedNode::new_unchecked(
                "http://www.w3.org/2002/07/owl#versionInfo"
            ))
            .map(String::as_str),
        Some("\"1.4.4\"")
    );
}
