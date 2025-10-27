//! Defines traits and implementations for handling graph input/output operations.
//! This includes reading graphs from files and URLs, and interacting with persistent or in-memory stores.

use crate::errors::OfflineRetrievalError;
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use crate::options::Overwrite;
use crate::util::get_file_contents;
use anyhow::{anyhow, Error, Result};
use chrono::prelude::*;
use fs2::FileExt;
use log::{error, info};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{Dataset, Graph, GraphName, GraphNameRef, NamedNode, NamedOrBlankNode, Quad};
use oxigraph::store::Store;
use rdf5d::{
    reader::R5tuFile,
    writer::{Quint, StreamingWriter, Term as R5Term, WriterOptions},
};
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct StoreStats {
    pub num_graphs: usize,
    pub num_triples: usize,
}

fn load_staging_store_from_bytes(bytes: &[u8], preferred: Option<RdfFormat>) -> Result<Store> {
    // Try preferred first, then fall back to other formats with a fresh store each time
    let mut candidates = vec![RdfFormat::Turtle, RdfFormat::RdfXml, RdfFormat::NTriples];
    if let Some(p) = preferred {
        candidates.retain(|f| *f != p);
        candidates.insert(0, p);
    }
    for fmt in candidates {
        let store = Store::new()?;
        let staging_graph = NamedNode::new_unchecked("temp:graph");
        let parser = RdfParser::from_format(fmt)
            .with_default_graph(GraphNameRef::NamedNode(staging_graph.as_ref()))
            .without_named_graphs();
        let mut loader = store.bulk_loader();
        match loader.load_from_reader(parser, std::io::Cursor::new(bytes)) {
            Ok(_) => {
                loader.commit()?;
                return Ok(store);
            }
            Err(_) => continue,
        }
    }
    Err(anyhow!("Failed to parse RDF bytes in any supported format"))
}

fn add_ontology_bytes(
    store: &Store,
    location: &OntologyLocation,
    bytes: &[u8],
    format: Option<RdfFormat>,
    overwrite: Overwrite,
    strict: bool,
) -> Result<Ontology> {
    let staging_graph = NamedNode::new_unchecked("temp:graph");
    let tmp_store = load_staging_store_from_bytes(bytes, format)?;
    let staging_id = GraphIdentifier::new_with_location(staging_graph.as_ref(), location.clone());
    let mut ontology = Ontology::from_store(&tmp_store, &staging_id, strict)?;
    ontology.with_last_updated(Utc::now());
    let id = ontology.id();
    let graphname: GraphName = id.graphname()?;

    if overwrite.as_bool() || !store.contains_named_graph(id.name())? {
        store.remove_named_graph(id.name())?;
        let quads = tmp_store
            .quads_for_pattern(
                None,
                None,
                None,
                Some(GraphNameRef::NamedNode(staging_graph.as_ref())),
            )
            .map(|res| res.map(|q| Quad::new(q.subject, q.predicate, q.object, graphname.clone())));
        let mut loader = store.bulk_loader();
        loader.load_ok_quads::<_, oxigraph::store::StorageError>(quads)?;
        loader.commit()?;
        info!("Added graph {} (from bytes)", id.name());
    }

    Ok(ontology)
}

/// A helper function to read an ontology from a location, add it to a store,
/// and return the parsed ontology metadata. This is used by multiple GraphIO implementations.
fn add_ontology_to_store(
    store: &Store,
    location: OntologyLocation,
    overwrite: Overwrite,
    offline: bool,
    strict: bool,
) -> Result<Ontology> {
    let (bytes, format) = match &location {
        OntologyLocation::File(path) => get_file_contents(path)?,
        OntologyLocation::Url(url) => {
            if offline {
                return Err(Error::new(OfflineRetrievalError { file: url.clone() }));
            }
            let opts = crate::fetch::FetchOptions::default();
            let fetched = crate::fetch::fetch_rdf(url.as_str(), &opts)?;
            (fetched.bytes, fetched.format)
        }
    };
    add_ontology_bytes(store, &location, &bytes, format, overwrite, strict)
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

    /// Adds a graph to the store and returns the ontology metadata.
    /// Existing graphs are replaced only when `overwrite` allows it.
    fn add(&mut self, location: OntologyLocation, overwrite: Overwrite) -> Result<Ontology>;

    /// Adds a graph to the store using pre-fetched bytes and optional format.
    fn add_from_bytes(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
        overwrite: Overwrite,
    ) -> Result<Ontology>;

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

    /// Begin a batch of mutations; default implementation is a no-op.
    fn begin_batch(&mut self) -> Result<()> {
        Ok(())
    }

    /// End a batch of mutations; default implementation is a no-op.
    fn end_batch(&mut self) -> Result<()> {
        Ok(())
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
                let opts = crate::fetch::FetchOptions::default();
                match crate::fetch::head_last_modified(url, &opts)? {
                    Some(dt) => dt,
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
    // Keep the interprocess lock alive for the lifetime of this IO
    lock_file: File,
    dirty: bool,
    batch_depth: usize,
}

impl PersistentGraphIO {
    pub fn new(path: PathBuf, offline: bool, strict: bool) -> Result<Self> {
        // Ensure target directory exists before creating/locking files
        std::fs::create_dir_all(&path)?;
        // Try to acquire an exclusive lock for writer; if any readers/writers hold the lock, error out immediately
        let lock_path = path.join("store.lock");
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        if let Err(e) = lock_file.try_lock_exclusive() {
            return Err(anyhow!(
                "Failed to open OntoEnv store for write: could not acquire exclusive lock on {:?}: {}. If another process has the store open (even read-only), open this instance in read-only mode.",
                lock_path, e
            ));
        }
        // Small delay to ensure lock contention is observable in concurrent tests/processes.
        // Keeps the lock held a bit longer so another writer will see it.
        std::thread::sleep(std::time::Duration::from_millis(75));
        // On-disk file is an RDF5D `.r5tu` file; in-memory store is Oxigraph
        let store_path = path.join("store.r5tu");
        let store = Store::new()?;
        // Load existing store from RDF5D file if it exists
        if store_path.exists() {
            Self::load_r5tu_into_store(&store, &store_path)?;
        }

        Ok(Self {
            store,
            offline,
            strict,
            store_path,
            lock_file,
            dirty: false,
            batch_depth: 0,
        })
    }

    fn load_r5tu_into_store(store: &Store, r5tu_path: &Path) -> Result<()> {
        let file = R5tuFile::open(r5tu_path)?;
        // Enumerate all logical graphs and load triples into named graphs
        let mut loader = store.bulk_loader();
        for gr in file.enumerate_all()? {
            let gname_str = gr.graphname;
            let gnn = NamedNode::new(&gname_str)
                .map_err(|e| anyhow!("Invalid graph name IRI in RDF5D: {}", e))?;
            let graphname = GraphName::NamedNode(gnn);
            // Iterate triples as Oxigraph terms (requires rdf5d `oxigraph` feature)
            let triples = file.oxigraph_triples(gr.gid)?;
            let mut quads_buf: Vec<Quad> = Vec::with_capacity(gr.n_triples as usize);
            for res in triples {
                let t = res.map_err(|e| anyhow!("RDF5D read error: {}", e))?;
                quads_buf.push(Quad::new(
                    t.subject,
                    t.predicate,
                    t.object,
                    graphname.clone(),
                ));
            }
            loader.load_quads(quads_buf.into_iter())?;
        }
        loader.commit()?;
        Ok(())
    }

    fn write_store_to_r5tu(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        // Stream out all quads in the in-memory store to an RDF5D file atomically
        let opts = WriterOptions {
            zstd: true,
            with_crc: true,
        };
        let mut writer = StreamingWriter::new(&self.store_path, opts);

        let iter = self.store.quads_for_pattern(None, None, None, None);
        for q in iter {
            let q = q?;
            // Dataset id: reuse graph name string; Graph name: same string
            let gname_str = match q.graph_name {
                oxigraph::model::GraphName::NamedNode(ref nn) => nn.as_str().to_string(),
                _ => return Err(anyhow!("Only named graphs are supported in RDF5D backend")),
            };
            let id_str = gname_str.clone();

            // Map Oxigraph terms to rdf5d writer terms
            let s_term = match q.subject {
                NamedOrBlankNode::NamedNode(nn) => R5Term::Iri(nn.as_str().to_string()),
                NamedOrBlankNode::BlankNode(bn) => R5Term::BNode(bn.as_str().to_string()),
            };
            let p_term = R5Term::Iri(q.predicate.as_str().to_string());
            let o_term = match q.object {
                oxigraph::model::Term::NamedNode(nn) => R5Term::Iri(nn.as_str().to_string()),
                oxigraph::model::Term::BlankNode(bn) => R5Term::BNode(bn.as_str().to_string()),
                oxigraph::model::Term::Literal(lit) => {
                    let lex = lit.value().to_string();
                    if let Some(lang) = lit.language() {
                        R5Term::Literal {
                            lex,
                            dt: None,
                            lang: Some(lang.to_string()),
                        }
                    } else {
                        let dt = lit.datatype().as_str().to_string();
                        R5Term::Literal {
                            lex,
                            dt: Some(dt),
                            lang: None,
                        }
                    }
                }
            };

            writer.add(Quint {
                id: id_str,
                s: s_term,
                p: p_term,
                o: o_term,
                gname: gname_str,
            })?;
        }

        writer.finalize()?;
        self.dirty = false;
        Ok(())
    }

    fn on_store_mutated(&mut self) -> Result<()> {
        self.dirty = true;
        if self.batch_depth == 0 {
            self.write_store_to_r5tu()?;
        }
        Ok(())
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

    fn add(&mut self, location: OntologyLocation, overwrite: Overwrite) -> Result<Ontology> {
        let ont =
            add_ontology_to_store(&self.store, location, overwrite, self.offline, self.strict)?;
        self.on_store_mutated()?;
        Ok(ont)
    }

    fn add_from_bytes(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
        overwrite: Overwrite,
    ) -> Result<Ontology> {
        let ont = add_ontology_bytes(
            &self.store,
            &location,
            &bytes,
            format,
            overwrite,
            self.strict,
        )?;
        self.on_store_mutated()?;
        Ok(ont)
    }

    fn remove(&mut self, id: &GraphIdentifier) -> Result<()> {
        let graphname = id.name();
        self.store.remove_named_graph(graphname)?;
        self.on_store_mutated()?;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.write_store_to_r5tu()
    }

    fn begin_batch(&mut self) -> Result<()> {
        self.batch_depth = self.batch_depth.saturating_add(1);
        Ok(())
    }

    fn end_batch(&mut self) -> Result<()> {
        if self.batch_depth == 0 {
            return Err(anyhow!("end_batch called without begin_batch"));
        }
        self.batch_depth -= 1;
        if self.batch_depth == 0 && self.dirty {
            self.write_store_to_r5tu()?;
        }
        Ok(())
    }

    fn size(&self) -> Result<StoreStats> {
        // Prefer reading stats directly from the RDF5D file without touching the in-memory store
        if !self.store_path.exists() {
            return Ok(StoreStats {
                num_graphs: 0,
                num_triples: 0,
            });
        }
        let f = R5tuFile::open(&self.store_path)?;
        let graphs = f.enumerate_all()?;
        let num_graphs = graphs.len();
        let num_triples: usize = graphs.iter().map(|gr| gr.n_triples as usize).sum();
        Ok(StoreStats {
            num_graphs,
            num_triples,
        })
    }
}

pub struct ReadOnlyPersistentGraphIO {
    store: Store,
    offline: bool,
    store_path: PathBuf,
    // Keep the shared interprocess lock alive for the lifetime of this IO
    lock_file: File,
}

impl ReadOnlyPersistentGraphIO {
    pub fn new(path: PathBuf, offline: bool) -> Result<Self> {
        // Acquire shared lock for readers; will block while a writer holds the exclusive lock
        let lock_path = path.join("store.lock");
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        lock_file.lock_shared()?;
        let store_path = path.join("store.r5tu");
        let store = Store::new()?;
        if store_path.exists() {
            PersistentGraphIO::load_r5tu_into_store(&store, &store_path)?;
        }
        Ok(Self {
            store,
            offline,
            store_path,
            lock_file,
        })
    }
}

impl Drop for PersistentGraphIO {
    fn drop(&mut self) {
        if self.dirty {
            if let Err(err) = self.write_store_to_r5tu() {
                error!("Failed to flush RDF5D store on drop: {err}");
            }
        }
        // Best-effort unlock on drop
        let _ = self.lock_file.unlock();
    }
}

impl Drop for ReadOnlyPersistentGraphIO {
    fn drop(&mut self) {
        // Best-effort unlock on drop
        let _ = self.lock_file.unlock();
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

    fn add(&mut self, _location: OntologyLocation, _overwrite: Overwrite) -> Result<Ontology> {
        Err(anyhow!("Cannot add to read-only store"))
    }

    fn add_from_bytes(
        &mut self,
        _location: OntologyLocation,
        _bytes: Vec<u8>,
        _format: Option<RdfFormat>,
        _overwrite: Overwrite,
    ) -> Result<Ontology> {
        Err(anyhow!("Cannot add to read-only store"))
    }

    fn remove(&mut self, _id: &GraphIdentifier) -> Result<()> {
        Err(anyhow!("Cannot remove from read-only store"))
    }

    fn size(&self) -> Result<StoreStats> {
        if !self.store_path.exists() {
            return Ok(StoreStats {
                num_graphs: 0,
                num_triples: 0,
            });
        }
        let f = R5tuFile::open(&self.store_path)?;
        let graphs = f.enumerate_all()?;
        let num_graphs = graphs.len();
        let num_triples: usize = graphs.iter().map(|gr| gr.n_triples as usize).sum();
        Ok(StoreStats {
            num_graphs,
            num_triples,
        })
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

    fn add(&mut self, location: OntologyLocation, overwrite: Overwrite) -> Result<Ontology> {
        add_ontology_to_store(&self.store, location, overwrite, self.offline, self.strict)
    }

    fn add_from_bytes(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
        overwrite: Overwrite,
    ) -> Result<Ontology> {
        add_ontology_bytes(
            &self.store,
            &location,
            &bytes,
            format,
            overwrite,
            self.strict,
        )
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
        let mut loader = self.store.bulk_loader();
        loader.load_quads(
            graph
                .iter()
                .map(|t| Quad::new(t.subject, t.predicate, t.object, graphname.clone())),
        )?;
        loader.commit()?;
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

    fn add(&mut self, location: OntologyLocation, overwrite: Overwrite) -> Result<Ontology> {
        add_ontology_to_store(&self.store, location, overwrite, self.offline, self.strict)
    }

    fn add_from_bytes(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
        overwrite: Overwrite,
    ) -> Result<Ontology> {
        add_ontology_bytes(
            &self.store,
            &location,
            &bytes,
            format,
            overwrite,
            self.strict,
        )
    }
}
