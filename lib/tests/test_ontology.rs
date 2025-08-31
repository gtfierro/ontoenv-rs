use ontoenv::ontology::OntologyLocation;
use oxigraph::model::NamedNode;

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
    let url = "http://example.com/ontology.ttl";
    let file = "/tmp/ontology.ttl";
    let url_location = OntologyLocation::from_str(url).unwrap();
    let file_location = OntologyLocation::from_str(file).unwrap();
    assert_eq!(url_location.to_string(), url);
    assert_eq!(file_location.to_string(), format!("file://{}", file));
}

#[test]
fn test_ontology_location_to_iri() {
    let url = "http://example.com/ontology.ttl";
    let file = "/tmp/ontology.ttl";
    let url_location = OntologyLocation::from_str(url).unwrap();
    let file_location = OntologyLocation::from_str(file).unwrap();
    assert_eq!(url_location.to_iri(), NamedNode::new(url).unwrap());
    assert_eq!(
        file_location.to_iri(),
        NamedNode::new(format!("file://{}", file)).unwrap()
    );
}
