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

use log::{debug, info, warn};

pub fn get_file_contents(path: &Path) -> Result<(Vec<u8>, Option<RdfFormat>)> {
    let b = std::fs::read(path)?;
    let format = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| match ext {
            "ttl" => Some(RdfFormat::Turtle),
            "xml" => Some(RdfFormat::RdfXml),
            "n3" => Some(RdfFormat::Turtle),
            "nt" => Some(RdfFormat::NTriples),
            _ => None,
        });
    Ok((b, format))
}

pub fn get_url_contents(url: &str) -> Result<(Vec<u8>, Option<RdfFormat>)> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(url)
        .header(CONTENT_TYPE, "application/x-turtle")
        .send()?;
    if !resp.status().is_success() {
        warn!("Failed to fetch ontology from {} ({})", url, resp.status());
        return Err(anyhow::anyhow!(
            "Failed to fetch ontology from {} ({})",
            url,
            resp.status()
        ));
    }
    let content_type = resp.headers().get("Content-Type");
    let format = content_type
        .and_then(|ct| ct.to_str().ok())
        .and_then(|ext| match ext {
            "application/x-turtle" => Some(RdfFormat::Turtle),
            "text/turtle" => Some(RdfFormat::Turtle),
            "application/rdf+xml" => Some(RdfFormat::RdfXml),
            "text/rdf+n3" => Some(RdfFormat::NTriples),
            _ => {
                debug!("Unknown content type: {ext}");
                None
            }
        });
    Ok((resp.bytes()?.to_vec(), format))
}

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
    debug!("Reading url: {file}");

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(file)
        .header(CONTENT_TYPE, "application/x-turtle")
        .send()?;
    if !resp.status().is_success() {
        warn!("Failed to fetch ontology from {} ({})", file, resp.status());
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
            debug!("Unknown content type: {ext}");
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
