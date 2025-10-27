use ontoenv::ontology::OntologyLocation;
use oxigraph::model::NamedNode;
use url::Url;

#[test]
fn test_ontology_location() {
    let url = "http://example.com/ontology.ttl";
    let file = "/tmp/ontology.ttl";
    let url_location = OntologyLocation::from_str(url).unwrap();
    let file_location = OntologyLocation::from_str(file).unwrap();
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
