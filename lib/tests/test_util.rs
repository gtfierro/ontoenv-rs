use ontoenv::util::{read_file, read_url, write_dataset_to_file};
use oxigraph::model::{Dataset, GraphNameRef, NamedNodeRef, QuadRef};
use std::path::Path;

#[test]
fn test_read_file() {
    // testing turtle file
    let graph = read_file(Path::new("fixtures/fileendings/model.ttl")).unwrap();
    assert_eq!(graph.len(), 5);

    // testing ntriples file
    let graph = read_file(Path::new("fixtures/fileendings/model.nt")).unwrap();
    assert_eq!(graph.len(), 5);

    // testing n3 file
    let graph = read_file(Path::new("fixtures/fileendings/model.n3")).unwrap();
    assert_eq!(graph.len(), 5);
    //
    // testing xml file
    let graph = read_file(Path::new("fixtures/fileendings/model.xml")).unwrap();
    assert_eq!(graph.len(), 5);

    // testing default turtle file
    let graph = read_file(Path::new("fixtures/fileendings/model")).unwrap();
    assert_eq!(graph.len(), 5);

    // reading non-existent file should return an error
    let result = read_file(Path::new("fixtures/data/non-existent.ttl"));
    assert!(result.is_err());
}

#[test]
fn test_read_url() {
    let graph =
        read_url("https://github.com/BrickSchema/Brick/releases/download/v1.4.0-rc1/Brick.ttl")
            .unwrap();
    assert_eq!(graph.len(), 53478);

    // reading non-existent url should return an error
    let result = read_url("http://example.org/non-existent.ttl");
    assert!(result.is_err());
}

#[test]
fn test_write_dataset_to_file() {
    // create in-memory dataset
    let mut graph = Dataset::new();
    let model = read_file(Path::new("fixtures/fileendings/model.ttl")).unwrap();
    let model_name =
        GraphNameRef::NamedNode(NamedNodeRef::new("http://example.org/model").unwrap());
    let brick = read_file(Path::new("fixtures/brick-stuff/Brick-1.3.ttl")).unwrap();
    let brick_name =
        GraphNameRef::NamedNode(NamedNodeRef::new("http://example.org/brick").unwrap());
    for quad in model.iter() {
        graph.insert(QuadRef::new(
            quad.subject,
            quad.predicate,
            quad.object,
            model_name,
        ));
    }
    for quad in brick.iter() {
        graph.insert(QuadRef::new(
            quad.subject,
            quad.predicate,
            quad.object,
            brick_name,
        ));
    }

    write_dataset_to_file(&graph, "model_out.ttl").unwrap();
}
