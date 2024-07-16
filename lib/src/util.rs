use anyhow::Result;

use std::io::Read;
use std::path::Path;

use reqwest::header::CONTENT_TYPE;

use oxigraph::io::write::GraphSerializer;
use oxigraph::io::{GraphFormat, GraphParser};
use oxigraph::model::graph::Graph as OxigraphGraph;
use oxigraph::model::Dataset;
use oxigraph::model::TripleRef;

use std::io::BufReader;

use log::{debug, info};

pub fn write_dataset_to_file(dataset: &Dataset, file: &str) -> Result<()> {
    info!(
        "Writing dataset to file: {} with length {}",
        file,
        dataset.len()
    );
    let mut file = std::fs::File::create(file)?;
    let mut serializer =
        GraphSerializer::from_format(GraphFormat::Turtle).triple_writer(&mut file);
    for quad in dataset.iter() {
        serializer.write(TripleRef {
            subject: quad.subject,
            predicate: quad.predicate,
            object: quad.object,
        })?;
    }
    serializer.finish()?;
    Ok(())
}

pub fn read_file(file: &Path) -> Result<OxigraphGraph> {
    debug!("Reading file: {}", file.to_str().unwrap());
    let filename = file;
    let file = std::fs::File::open(file)?;
    let content: BufReader<_> = BufReader::new(file);
    let content_type = filename.extension().and_then(|ext| ext.to_str());
    let content_type = content_type.and_then(|ext| match ext {
        "ttl" => Some(GraphFormat::Turtle),
        "xml" => Some(GraphFormat::RdfXml),
        "n3" => Some(GraphFormat::Turtle),
        "nt" => Some(GraphFormat::NTriples),
        _ => None,
    });
    let parser = GraphParser::from_format(content_type.unwrap_or(GraphFormat::Turtle));
    let mut graph = OxigraphGraph::new();
    let triples = parser.read_triples(content);
    for triple in triples {
        graph.insert(&triple?);
    }

    Ok(graph)
}

pub fn read_url(file: &str) -> Result<OxigraphGraph> {
    debug!("Reading url: {}", file);

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(file)
        .header(CONTENT_TYPE, "application/x-turtle")
        .send()?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Failed to fetch ontology from {}", file));
    }
    let content_type = resp.headers().get("Content-Type");
    let content_type = content_type.and_then(|ct| ct.to_str().ok());
    let content_type = content_type.and_then(|ext| match ext {
        "application/x-turtle" => Some(GraphFormat::Turtle),
        "application/rdf+xml" => Some(GraphFormat::RdfXml),
        "text/rdf+n3" => Some(GraphFormat::NTriples),
        _ => {
            debug!("Unknown content type: {}", ext);
            None
        }
    });

    let content: BufReader<_> = BufReader::new(std::io::Cursor::new(resp.bytes()?));

    // if content type is known, use it to parse the graph
    if let Some(format) = content_type {
        let parser = GraphParser::from_format(format);
        let mut graph = OxigraphGraph::new();
        let triples = parser.read_triples(content);
        for triple in triples {
            graph.insert(&triple?);
        }
        return Ok(graph);
    }

    // if content  type is unknown, try all formats. Requires us to make a copy of the content
    // since we can't rewind the reader
    let content_vec: Vec<u8> = content.bytes().map(|b| b.unwrap()).collect();

    for format in [
        GraphFormat::Turtle,
        GraphFormat::RdfXml,
        GraphFormat::NTriples,
    ] {
        let vcontent = BufReader::new(std::io::Cursor::new(&content_vec));
        let parser = GraphParser::from_format(format);
        let mut graph = OxigraphGraph::new();

        // if there's an error on parser.read_triples, try the next format

        for triple in parser.read_triples(vcontent) {
            graph.insert(&triple?);
        }
        return Ok(graph);
    }
    Err(anyhow::anyhow!("Failed to parse graph from {}", file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::model::{Dataset, GraphNameRef, NamedNodeRef, QuadRef};

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
}
