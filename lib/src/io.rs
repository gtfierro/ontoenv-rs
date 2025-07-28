//! Defines traits and implementations for handling graph input/output operations.
//! This includes reading graphs from files and URLs, and interacting with persistent or in-memory stores.

use crate::errors::OfflineRetrievalError;
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use crate::util::{get_file_contents, get_url_contents};
use anyhow::{anyhow, Error, Result};
use chrono::prelude::*;
use log::{debug, info};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{Dataset, Graph, GraphName, GraphNameRef, NamedNode, Quad};
use oxigraph::store::Store;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct StoreStats {
    pub num_graphs: usize,
    pub num_triples: usize,
}

/// A helper function to read an ontology from a location, add it to a store,
/// and return the parsed ontology metadata. This is used by multiple GraphIO implementations.
fn add_ontology_to_store(
    store: &Store,
    location: OntologyLocation,
    overwrite: bool,
    offline: bool,
    strict: bool,
) -> Result<Ontology> {
    // 1. Get content into bytes and determine format
    let (bytes, format) = match &location {
        OntologyLocation::File(path) => get_file_contents(path)?,
        OntologyLocation::Url(url) => {
            if offline {
                return Err(Error::new(OfflineRetrievalError {
                    file: url.clone(),
                }));
            }
            get_url_contents(url.as_str())?
        }
    };

    let temp_graph_name = NamedNode::new_unchecked("temp:graph");
    if store.contains_named_graph(temp_graph_name.as_ref())? {
        store.remove_named_graph(temp_graph_name.as_ref())?;
    }
    let parser = RdfParser::from_format(format.unwrap_or(RdfFormat::Turtle))
        .with_default_graph(GraphNameRef::NamedNode(temp_graph_name.as_ref()))
        .without_named_graphs();
    let now = Instant::now();
    store
        .bulk_loader()
        .load_from_reader(parser, bytes.as_slice())?;
    info!(
        "Bulk loaded {} into temp graph in {:?}",
        location.as_str(),
        now.elapsed()
    );
    let temp_graph_id = GraphIdentifier::new_with_location(temp_graph_name.as_ref(), location);
    let mut ontology = Ontology::from_store(store, &temp_graph_id, strict)?;

    debug!("Adding ontology: {}", ontology.id());
    ontology.with_last_updated(Utc::now());
    let id = ontology.id();
    let graphname: GraphName = id.graphname()?;

    // 3. Load from bytes using bulk loader
    if overwrite || !store.contains_named_graph(id.name())? {
        store.remove_named_graph(id.name())?;
        let now = Instant::now();
        let quads_to_load = store
            .quads_for_pattern(
                None,
                None,
                None,
                Some(GraphNameRef::NamedNode(temp_graph_name.as_ref())),
            )
            .map(|res| {
                res.map(|q| Quad::new(q.subject, q.predicate, q.object, graphname.clone()))
            });
        debug!("Loading quads into graph {}", id);
        store
            .bulk_loader()
            .load_ok_quads::<_, oxigraph::store::StorageError>(quads_to_load)?;
        info!(
            "Copied temp graph to {} in {:?}",
            id.name(),
            now.elapsed()
        );
    }
    store.remove_named_graph(temp_graph_name.as_ref())?;
    Ok(ontology)
}

pub trait GraphIO: Send + Sync {
    /// Returns true if the store is offline; if this is true, then the store
    /// will not fetch any data from the internet
    fn is_offline(&self) -> bool;

    /// Returns the type of the store (e.g., "persistent", "memory", "read-only")
    fn io_type(&self) -> String;

    /// Returns the path to the store, if it is a file-based store
    fn store_location(&self) -> Option<&Path>;

    /// Returns a reference to the underlying store
    fn store(&self) -> &Store;

    /// Adds a graph to the store and returns the ontology metadata. Overwrites any existing graph with
    /// the same identifier if 'overwrite' is true.
    fn add(&mut self, location: OntologyLocation, overwrite: bool) -> Result<Ontology>;

    /// Returns the graph with the given identifier
    fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        let mut graph = Graph::new();
        let graphname = id.graphname()?;
        for quad in self
            .store()
            .quads_for_pattern(None, None, None, Some(graphname.as_ref()))
        {
            graph.insert(quad?.as_ref());
        }
        Ok(graph)
    }

    /// Returns the size of the underlying store.
    fn size(&self) -> Result<StoreStats> {
        let num_graphs = self.store().named_graphs().count();
        let num_triples = self.store().len()?;
        Ok(StoreStats {
            num_graphs,
            num_triples,
        })
    }

    /// Removes the graph with the given identifier from the store and ontology metadata
    fn remove(&mut self, id: &GraphIdentifier) -> Result<()> {
        let graphname = id.name();
        self.store().remove_named_graph(graphname)?;
        Ok(())
    }

    /// Returns the union of the graphs with the given identifiers
    fn union_graph(&self, ids: &[GraphIdentifier]) -> Dataset {
        let mut graph = Dataset::new();
        for id in ids {
            let graphname = id.graphname().unwrap();
            let g = self.get_graph(id).unwrap();
            for t in g.iter() {
                graph.insert(&Quad::new(
                    t.subject,
                    t.predicate,
                    t.object,
                    graphname.clone(),
                ));
            }
        }
        graph
    }

    fn flush(&mut self) -> Result<()> {
        self.store()
            .flush()
            .map_err(|e| anyhow!("Failed to flush store: {}", e))
    }

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

    fn read_file(&self, file: &Path) -> Result<Graph> {
        crate::util::read_file(file)
    }

    fn read_url(&self, file: &str) -> Result<Graph> {
        crate::util::read_url(file)
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

    fn io_type(&self) -> String {
        "persistent".to_string()
    }

    fn store_location(&self) -> Option<&Path> {
        Some(&self.store_path)
    }

    fn store(&self) -> &Store {
        &self.store
    }

    fn add(&mut self, location: OntologyLocation, overwrite: bool) -> Result<Ontology> {
        add_ontology_to_store(&self.store, location, overwrite, self.offline, self.strict)
    }
}

pub struct ReadOnlyPersistentGraphIO {
    store: Store,
    offline: bool,
    store_path: PathBuf,
}

impl ReadOnlyPersistentGraphIO {
    pub fn new(path: PathBuf, offline: bool) -> Result<Self> {
        let store_path = path.join("store.db");
        let store = Store::open_read_only(store_path.clone())?;
        Ok(Self {
            store,
            offline,
            store_path,
        })
    }
}

impl GraphIO for ReadOnlyPersistentGraphIO {
    fn is_offline(&self) -> bool {
        self.offline
    }

    fn io_type(&self) -> String {
        "read-only".to_string()
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }

    fn store_location(&self) -> Option<&Path> {
        Some(&self.store_path)
    }

    fn store(&self) -> &Store {
        &self.store
    }

    fn add(&mut self, _location: OntologyLocation, _overwrite: bool) -> Result<Ontology> {
        Err(anyhow!("Cannot add to read-only store"))
    }

    fn remove(&mut self, _id: &GraphIdentifier) -> Result<()> {
        Err(anyhow!("Cannot remove from read-only store"))
    }
}

pub struct ExternalStoreGraphIO {
    store: Store,
    offline: bool,
    strict: bool,
}

impl ExternalStoreGraphIO {
    pub fn new(store: Store, offline: bool, strict: bool) -> Self {
        Self {
            store,
            offline,
            strict,
        }
    }
}

impl GraphIO for ExternalStoreGraphIO {
    fn is_offline(&self) -> bool {
        self.offline
    }

    fn io_type(&self) -> String {
        "external-store".to_string()
    }

    fn store_location(&self) -> Option<&Path> {
        None
    }

    fn store(&self) -> &Store {
        &self.store
    }

    fn add(&mut self, location: OntologyLocation, overwrite: bool) -> Result<Ontology> {
        add_ontology_to_store(&self.store, location, overwrite, self.offline, self.strict)
    }
}

pub struct MemoryGraphIO {
    store: Store,
    offline: bool,
    strict: bool,
}

impl MemoryGraphIO {
    pub fn new(offline: bool, strict: bool) -> Result<Self> {
        Ok(Self {
            store: Store::new()?,
            offline,
            strict,
        })
    }

    pub fn add_graph(&mut self, id: GraphIdentifier, graph: Graph) -> Result<()> {
        let graphname = id.graphname()?;
        self.store.remove_named_graph(id.name())?;
        self.store.bulk_loader().load_quads(graph.iter().map(|t| {
            Quad::new(
                t.subject,
                t.predicate,
                t.object,
                graphname.clone(),
            )
        }))?;
        Ok(())
    }
}

impl GraphIO for MemoryGraphIO {
    fn is_offline(&self) -> bool {
        self.offline
    }

    fn io_type(&self) -> String {
        "memory".to_string()
    }

    fn store_location(&self) -> Option<&Path> {
        None
    }

    fn store(&self) -> &Store {
        &self.store
    }

    fn add(&mut self, location: OntologyLocation, overwrite: bool) -> Result<Ontology> {
        add_ontology_to_store(&self.store, location, overwrite, self.offline, self.strict)
    }
}
