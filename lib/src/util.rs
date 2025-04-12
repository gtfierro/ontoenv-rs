//! Provides utility functions for common tasks within the OntoEnv library,
//! such as reading/writing RDF files and URLs, and converting between graph representations.

use anyhow::Result;

use std::io::{Read, Seek};
use std::path::Path;

use reqwest::header::CONTENT_TYPE;

use oxigraph::io::{RdfFormat, RdfParser, RdfSerializer};
use oxigraph::model::graph::Graph as OxigraphGraph;
use oxigraph::model::Dataset;
use oxigraph::model::{GraphNameRef, Quad, Triple, TripleRef};

use std::io::BufReader;

use log::{debug, error, info};

pub fn write_dataset_to_file(dataset: &Dataset, file: &str) -> Result<()> {
    info!(
        "Writing dataset to file: {} with length {}",
        file,
        dataset.len()
    );
    let mut file = std::fs::File::create(file)?;
    let mut serializer = RdfSerializer::from_format(RdfFormat::Turtle).for_writer(&mut file);
    for quad in dataset.iter() {
        serializer.serialize_triple(TripleRef {
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
        "ttl" => Some(RdfFormat::Turtle),
        "xml" => Some(RdfFormat::RdfXml),
        "n3" => Some(RdfFormat::Turtle),
        "nt" => Some(RdfFormat::NTriples),
        _ => None,
    });
    let parser = RdfParser::from_format(content_type.unwrap_or(RdfFormat::Turtle));
    let mut graph = OxigraphGraph::new();
    let parser = parser.for_reader(content);
    for quad in parser {
        let quad = quad?;
        let triple = Triple::new(quad.subject, quad.predicate, quad.object);
        graph.insert(&triple);
    }

    Ok(graph)
}

pub fn read_format<T: Read + Seek>(
    mut original_content: BufReader<T>,
    format: Option<RdfFormat>,
) -> Result<OxigraphGraph> {
    let format = format.unwrap_or(RdfFormat::Turtle);
    for format in [
        format,
        RdfFormat::Turtle,
        RdfFormat::RdfXml,
        RdfFormat::NTriples,
    ] {
        let content = original_content.get_mut();
        content.rewind()?;
        let parser = RdfParser::from_format(format);
        let mut graph = OxigraphGraph::new();
        let parser = parser.for_reader(content);

        // Process each quad from the parser
        for quad in parser {
            match quad {
                Ok(q) => {
                    let triple = Triple::new(q.subject, q.predicate, q.object);
                    graph.insert(&triple);
                }
                Err(_) => {
                    // Break the outer loop if an error occurs
                    break;
                }
            }
        }

        // If we successfully processed quads and did not encounter an error
        if !graph.is_empty() {
            return Ok(graph);
        }
    }
    Err(anyhow::anyhow!("Failed to parse graph"))
}

pub fn read_url(file: &str) -> Result<OxigraphGraph> {
    debug!("Reading url: {}", file);

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(file)
        .header(CONTENT_TYPE, "application/x-turtle")
        .send()?;
    if !resp.status().is_success() {
        error!("Failed to fetch ontology from {} ({})", file, resp.status());
        return Err(anyhow::anyhow!(
            "Failed to fetch ontology from {} ({})",
            file,
            resp.status()
        ));
    }
    let content_type = resp.headers().get("Content-Type");
    let content_type = content_type.and_then(|ct| ct.to_str().ok());
    let content_type = content_type.and_then(|ext| match ext {
        "application/x-turtle" => Some(RdfFormat::Turtle),
        "text/turtle" => Some(RdfFormat::Turtle),
        "application/rdf+xml" => Some(RdfFormat::RdfXml),
        "text/rdf+n3" => Some(RdfFormat::NTriples),
        _ => {
            debug!("Unknown content type: {}", ext);
            None
        }
    });

    let content: BufReader<_> = BufReader::new(std::io::Cursor::new(resp.bytes()?));
    read_format(content, content_type)
}

// return a "impl IntoIterator<Item = impl Into<Quad>>" for a graph. Iter through
// the input Graph and create a Quad for each Triple in the Graph using the given GraphName
pub fn graph_to_quads<'a>(
    graph: &'a OxigraphGraph,
    graph_name: GraphNameRef<'a>,
) -> impl IntoIterator<Item = impl Into<Quad> + use<'a>> {
    graph
        .into_iter()
        .map(move |triple| triple.in_graph(graph_name))
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
