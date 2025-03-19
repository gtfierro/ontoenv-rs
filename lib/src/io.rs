use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use crate::errors::OfflineRetrievalError;
use crate::util::read_format;
use anyhow::{anyhow, Result, Error};
use chrono::prelude::*;
use log::{debug, error};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::NamedOrBlankNode;
use oxigraph::model::{Dataset, Graph, GraphName, Quad, Triple};
use oxigraph::store::Store;
use reqwest::header::CONTENT_TYPE;
use std::collections::HashMap;
use std::io::BufReader;
use std::path::Path;
use std::path::PathBuf;

pub struct StoreStats {
    pub num_graphs: usize,
    pub num_triples: usize,
}

pub trait GraphIO: Send + Sync {
    /// Returns true if the store is offline; if this is true, then the store
    /// will not fetch any data from the internet

    fn is_offline(&self) -> bool;
    /// Returns the graph with the given identifier
    fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph>;

    /// Returns the path to the store, if it is a file-based store
    fn store_location(&self) -> Option<&Path>;

    /// Returns the size of the underlying store.
    fn size(&self) -> Result<StoreStats>;

    /// Adds a graph to the store and returns the ontology metadata. Overwrites any existing graph with
    /// the same identifier if 'overwrite' is true.
    fn add(&mut self, location: OntologyLocation, _overwrite: bool) -> Result<Ontology>;

    /// Removes the graph with the given identifier from the store and ontology metadata
    fn remove(&mut self, id: &GraphIdentifier) -> Result<()>;

    /// Returns the union of the graphs with the given identifiers
    fn union_graph(&self, ids: &[GraphIdentifier]) -> Dataset;

    /// Returns the last time the graph with the given identifier was modified at its location
    /// - for on-disk files (file://), if the file has been modified since the last refresh
    /// - for online files (http://), the file's header has a Last-Modified header with a later
    /// date than the last refresh. If there is no Last-Modified header, the store will always
    /// refresh the file.
    fn source_last_modified(&self, id: &GraphIdentifier) -> Result<DateTime<Utc>> {
        let modified_time = match id.location() {
            OntologyLocation::File(path) => {
                let metadata = std::fs::metadata(path)?;
                let modified: DateTime<Utc> = metadata.modified()?.into();
                modified
            }
            OntologyLocation::Url(url) => {
                let response = reqwest::blocking::Client::new().head(url).send()?;
                let url_last_modified = response.headers().get("Last-Modified");
                match url_last_modified {
                    Some(date) => {
                        let date = date.to_str()?;
                        let date = DateTime::parse_from_rfc2822(date)?;
                        date.with_timezone(&Utc)
                    }
                    None => Utc::now(),
                }
            }
        };
        Ok(modified_time)
    }

    // /// Refreshes the contents of each graph whose identifier is in the list.
    // /// Returns the new ontologies that were added to the store and the updated
    // /// ontology records. These all need to be added to the environment.
    // fn refresh(&mut self, env: &Environment, ids: Vec<GraphIdentifier>) -> Result<Vec<Ontology>> {
    //     let updated_ontologies: Vec<Ontology> = Vec::new();
    //     for id in ids {
    //         let ont = self.add(id.location().clone(), force)?;
    //         ont.last_updated = Some(Utc::now());
    //         if *ont.id() != id {
    //             // TODO: handle
    //             return Err(anyhow!("Refreshed graph has different identifier"));
    //         }
    //         updated_ontologies.push(ont);
    //     }
    //     Ok(updated_ontologies)
    // }

    fn read_file(&self, file: &Path) -> Result<Graph> {
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
        let mut graph = Graph::new();
        let parser = parser.for_reader(content);
        for quad in parser {
            let quad = quad?;
            let triple = Triple::new(quad.subject, quad.predicate, quad.object);
            graph.insert(&triple);
        }

        Ok(graph)
    }

    fn read_url(&self, file: &str) -> Result<Graph> {
        debug!("Reading url: {}", file);

        let client = reqwest::blocking::Client::new();
        let resp = client
            .get(file)
            .header(CONTENT_TYPE, "application/x-turtle")
            .send()?;
        if !resp.status().is_success() {
            error!("Failed to fetch ontology from {} ({})", file, resp.status());
            return Err(anyhow::anyhow!("Failed to fetch ontology from {} ({})", file, resp.status()));
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
}

pub struct PersistentGraphIO {
    store: Store,
    offline: bool,
    strict: bool,
    store_path: PathBuf,
}

impl PersistentGraphIO {
    pub fn new(path: PathBuf, offline: bool, strict: bool) -> Result<Self> {
        let store_path = path.join("store.db");
        let store = Store::open(store_path.clone())?;
        Ok(Self {
            store,
            offline,
            strict,
            store_path,
        })
    }
}

impl GraphIO for PersistentGraphIO {
    fn is_offline(&self) -> bool {
        self.offline
    }

    fn size(&self) -> Result<StoreStats> {
        let num_graphs = self.store.named_graphs().count();
        let num_triples = self.store.len()?;
        Ok(StoreStats {
            num_graphs,
            num_triples,
        })
    }

    fn store_location(&self) -> Option<&Path> {
        Some(&self.store_path)
    }

    fn union_graph(&self, ids: &[GraphIdentifier]) -> Dataset {
        let mut graph = Dataset::new();
        for id in ids {
            let graphname = id.graphname().unwrap();
            let g = self.get_graph(&id).unwrap();
            for t in g.iter() {
                graph.insert(&Quad::new(
                    t.subject.clone(),
                    t.predicate.clone(),
                    t.object.clone(),
                    graphname.clone(),
                ));
            }
        }
        graph
    }

    fn add(&mut self, location: OntologyLocation, overwrite: bool) -> Result<Ontology> {
        let graph = match location {
            OntologyLocation::File(ref path) => self.read_file(&path)?,
            OntologyLocation::Url(ref url) => if self.offline {
                return Err(Error::new(OfflineRetrievalError {
                    file: url.clone(),
                }))
            } else {
                self.read_url(&url)?
            },
        };

        let ontology = Ontology::from_graph(&graph, location.clone(), self.strict)?;
        let id = ontology.id().clone();

        let graphname: NamedOrBlankNode = match id.graphname()? {
            GraphName::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
            _ => return Err(anyhow!("Graph name not found")),
        };

        if overwrite || !self.store.contains_named_graph(graphname.as_ref())? {
            self.store.remove_named_graph(graphname.as_ref())?;
            self.store.bulk_loader().load_quads(graph.iter().map(|t| {
                Quad::new(
                    t.subject.clone(),
                    t.predicate.clone(),
                    t.object.clone(),
                    graphname.clone(),
                )
            }))?;
        }
        Ok(ontology)
    }

    fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        let mut graph = Graph::new();
        let graphname = id.graphname()?;
        for quad in self
            .store
            .quads_for_pattern(None, None, None, Some(graphname.as_ref()))
        {
            graph.insert(quad?.as_ref());
        }
        Ok(graph)
    }

    fn remove(&mut self, id: &GraphIdentifier) -> Result<()> {
        let graphname = id.name();
        self.store.remove_named_graph(graphname)?;
        Ok(())
    }
}

pub struct MemoryGraphIO {
    graphs: HashMap<GraphIdentifier, Graph>,
    offline: bool,
    strict: bool,
}

impl MemoryGraphIO {
    pub fn new(offline: bool, strict: bool) -> Self {
        Self {
            graphs: HashMap::new(),
            offline,
            strict,
        }
    }
}

impl GraphIO for MemoryGraphIO {
    fn is_offline(&self) -> bool {
        self.offline
    }

    fn store_location(&self) -> Option<&Path> {
        None
    }

    fn size(&self) -> Result<StoreStats> {
        let num_graphs = self.graphs.len();
        let num_triples = self
            .graphs
            .values()
            .map(|g| g.len())
            .fold(0, |acc, x| acc + x);
        Ok(StoreStats {
            num_graphs,
            num_triples,
        })
    }

    fn union_graph(&self, ids: &[GraphIdentifier]) -> Dataset {
        let mut graph = Dataset::new();
        for id in ids {
            let graphname = id.graphname().unwrap();
            let g = self.get_graph(&id).unwrap();
            for t in g.iter() {
                graph.insert(&Quad::new(
                    t.subject.clone(),
                    t.predicate.clone(),
                    t.object.clone(),
                    graphname.clone(),
                ));
            }
        }
        graph
    }

    fn add(&mut self, location: OntologyLocation, overwrite: bool) -> Result<Ontology> {
        let graph = match location {
            OntologyLocation::File(ref path) => self.read_file(&path)?,
            OntologyLocation::Url(ref url) => if self.offline {
                return Err(Error::new(OfflineRetrievalError {
                    file: url.clone(),
                }))
            } else {
                self.read_url(&url)?
            },
        };

        let ontology = Ontology::from_graph(&graph, location.clone(), self.strict)?;
        let id = ontology.id().clone();
        if overwrite || self.graphs.get(&id).is_none() {
            self.graphs.insert(id, graph);
        }
        Ok(ontology)
    }

    fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        Ok(self
            .graphs
            .get(&id)
            .ok_or(anyhow!("Graph not found"))?
            .clone())
    }

    fn remove(&mut self, id: &GraphIdentifier) -> Result<()> {
        self.graphs.remove(id);
        Ok(())
    }
}
