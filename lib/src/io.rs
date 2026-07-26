//! Defines traits and implementations for handling graph input/output operations.
//! This includes reading graphs from files and URLs, and interacting with persistent or in-memory stores.

use crate::catalog::BackendState;
use crate::errors::OfflineRetrievalError;
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use crate::options::Overwrite;
use crate::util::get_file_contents;
use crate::FailedImport;
use anyhow::{anyhow, Error, Result};
use blake3;
use chrono::prelude::*;
use log::{error, info};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{
    Dataset, Graph, GraphName, GraphNameRef, NamedNode, NamedOrBlankNode, Quad, QuadRef,
};
use oxigraph::store::Store;
use rdf5d::{
    reader::R5tuFile,
    writer::{Quint, StreamingWriter, Term as R5Term, WriterOptions},
};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct StoreStats {
    pub num_graphs: usize,
    pub num_triples: usize,
}

#[derive(Debug, Clone)]
struct R5GraphInfo {
    gid: u64,
    id: String,
    n_triples: u64,
}

/// Returns `<r5tu>.idx`, the path of the legacy on-disk index sidecar that
/// older versions wrote next to a snapshot. Query indexes are now built in
/// memory; this is only used to delete a stale sidecar left by an upgrade.
fn legacy_sidecar_path(r5tu: &Path) -> PathBuf {
    let mut s = r5tu.as_os_str().to_owned();
    s.push(".idx");
    PathBuf::from(s)
}

fn file_backend_state(path: &Path) -> Result<BackendState> {
    // The store file does not exist for a newly-created empty environment.
    // Canonicalize its existing parent so platform aliases such as macOS
    // `/var` -> `/private/var` still produce the same backend identity before
    // and after reopening through a differently-spelled path.
    let identity_path = match (path.parent(), path.file_name()) {
        (Some(parent), Some(file_name)) => {
            crate::ontology::canonicalize_file_path(parent).join(file_name)
        }
        _ => crate::ontology::canonicalize_file_path(path),
    };
    let identity = identity_path.to_string_lossy().into_owned();
    let revision = match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or_default();
            format!("{}:{modified}", metadata.len())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing".to_string(),
        Err(error) => return Err(error.into()),
    };
    Ok(BackendState {
        id: format!("rdf5d:{identity}"),
        revision,
    })
}

fn load_staging_store_from_bytes(bytes: &[u8], preferred: Option<RdfFormat>) -> Result<Store> {
    // Try multiple parsers to maximize compatibility with unknown RDF inputs.
    // Try preferred first, then fall back to all other formats.
    use oxigraph::io::JsonLdProfileSet;
    let mut candidates = vec![
        RdfFormat::Turtle,
        RdfFormat::RdfXml,
        RdfFormat::NTriples,
        RdfFormat::NQuads,
        RdfFormat::TriG,
        RdfFormat::JsonLd {
            profile: JsonLdProfileSet::default(),
        },
    ];
    if let Some(p) = preferred {
        candidates.retain(|f| *f != p);
        candidates.insert(0, p);
    }
    let store = Store::new()?;
    for fmt in candidates {
        // Load into a temporary named graph so the parser has a stable target.
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
    // Parse into a temporary store to extract ontology metadata safely.
    let staging_graph = NamedNode::new_unchecked("temp:graph");
    let tmp_store = load_staging_store_from_bytes(bytes, format)?;
    let staging_id = GraphIdentifier::new_with_location(staging_graph.as_ref(), location.clone());
    let mut ontology = Ontology::from_store(&tmp_store, &staging_id, strict)?;
    // Hash content for change detection without re-reading sources.
    let hash = blake3::hash(bytes).to_hex().to_string();
    ontology.set_content_hash(hash);
    ontology.with_last_updated(Utc::now());
    let id = ontology.id();
    let graphname: GraphName = id.graphname()?;

    // Only write into the store if overwrite is allowed or the graph is absent.
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
    // Resolve bytes from the location, honoring offline mode.
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
        OntologyLocation::InMemory { .. } => {
            return Err(anyhow!(
                "In-memory ontologies cannot be persisted or refreshed from a source"
            ))
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

    /// Return an opaque backend identity and global revision in O(1), when
    /// supported. The revision must change after every backend mutation.
    fn store_state(&self) -> Result<Option<BackendState>> {
        Ok(None)
    }

    /// Return opaque per-graph revisions, when supported.
    fn graph_revisions(&self) -> Result<Option<HashMap<String, String>>> {
        Ok(None)
    }

    /// Returns the identifiers of all graphs currently held in the store.
    fn graph_ids(&self) -> Result<Vec<GraphIdentifier>> {
        self.store()
            .named_graphs()
            .map(|r| {
                let named = r.map_err(|e| anyhow!(e.to_string()))?;
                match named {
                    NamedOrBlankNode::NamedNode(n) => Ok(GraphIdentifier::new(n.as_ref())),
                    NamedOrBlankNode::BlankNode(_) => {
                        Err(anyhow!("blank-node graph names are not supported"))
                    }
                }
            })
            .collect()
    }

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

    /// Write a pre-built graph into the store under the given identifier, replacing any
    /// existing graph at that name.  Used by the rename machinery after a transform.
    fn add_named_graph(&mut self, id: GraphIdentifier, graph: Graph) -> Result<()> {
        let graphname = id.graphname()?;
        self.store().remove_named_graph(id.name())?;
        let mut loader = self.store().bulk_loader();
        loader.load_quads(
            graph
                .iter()
                .map(|t| Quad::new(t.subject, t.predicate, t.object, graphname.clone())),
        )?;
        loader.commit()?;
        Ok(())
    }

    /// Hook for backends that lazy-load graphs from on-disk storage into the
    /// in-memory store on first access. Default is a no-op. Persistent backends
    /// override this so that callers iterating `store()` directly (e.g.
    /// `union_graph`) still see the graph's quads.
    fn ensure_loaded(&self, _id: &GraphIdentifier) -> Result<()> {
        Ok(())
    }

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

    /// Returns a copy of the graph for mutable operations.
    ///
    /// Backends that can provide a more efficient or semantically distinct
    /// copy path (e.g. a Python graph store that returns a cached view from
    /// `get_graph` but a fresh deep copy from `copy_graph`) should override
    /// this. The default delegates to `get_graph`.
    fn copy_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        self.get_graph(id)
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

    /// Returns the best-effort union of the graphs with the given identifiers,
    /// along with a list of ids that could not be read.
    ///
    /// A failure on any single id (bad graphname, ensure_loaded error, or a
    /// store iteration error mid-graph) is recorded in the returned
    /// `Vec<FailedImport>` and that id is skipped; the rest of the union is
    /// still assembled. Callers that need strict all-or-nothing semantics
    /// should check the failures list and error themselves.
    fn union_graph(&self, ids: &[GraphIdentifier]) -> (Dataset, Vec<FailedImport>) {
        // Stream quads from the store directly into the Dataset. The previous
        // implementation materialized an intermediate Graph per id, which paid
        // for an extra hashmap insert per triple and an N-graph allocation.
        let mut dataset = Dataset::new();
        let mut failures: Vec<FailedImport> = Vec::new();
        for id in ids {
            let graphname = match id.graphname() {
                Ok(gn) => gn,
                Err(e) => {
                    failures.push(FailedImport::new(id.clone(), e.to_string()));
                    continue;
                }
            };
            // For persistent backends, ensure the named graph is in the in-memory store.
            if let Err(e) = self.ensure_loaded(id) {
                failures.push(FailedImport::new(id.clone(), e.to_string()));
                continue;
            }
            let mut graph_failure: Option<Error> = None;
            for quad in self
                .store()
                .quads_for_pattern(None, None, None, Some(graphname.as_ref()))
            {
                match quad {
                    Ok(q) => {
                        dataset.insert(QuadRef::new(
                            q.subject.as_ref(),
                            q.predicate.as_ref(),
                            q.object.as_ref(),
                            graphname.as_ref(),
                        ));
                    }
                    Err(e) => {
                        graph_failure = Some(anyhow!("union_graph store error: {}", e));
                        break;
                    }
                }
            }
            if let Some(e) = graph_failure {
                failures.push(FailedImport::new(id.clone(), e.to_string()));
            }
        }
        (dataset, failures)
    }

    /// Returns the best-effort union used by read-only view construction.
    ///
    /// Most backends have identical read and copy semantics, so the default
    /// delegates to [`Self::union_graph`]. Backends that distinguish a live
    /// read view from a detached copy should override this method and assemble
    /// the union through [`Self::get_graph`].
    fn view_union_graph(&self, ids: &[GraphIdentifier]) -> (Dataset, Vec<FailedImport>) {
        self.union_graph(ids)
    }

    fn flush(&mut self) -> Result<()> {
        #[cfg(feature = "rocksdb")]
        return self
            .store()
            .flush()
            .map_err(|e| anyhow!("Failed to flush store: {}", e));
        #[cfg(not(feature = "rocksdb"))]
        Ok(())
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
    ///   date than the last refresh. If there is no Last-Modified header, the store will always
    ///   refresh the file.
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
            OntologyLocation::InMemory { .. } => {
                return Err(anyhow!(
                    "In-memory ontologies do not have a source modification time"
                ))
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

fn oxigraph_subject_to_r5term(s: NamedOrBlankNode) -> R5Term {
    match s {
        NamedOrBlankNode::NamedNode(nn) => R5Term::Iri(nn.as_str().to_string()),
        NamedOrBlankNode::BlankNode(bn) => R5Term::BNode(bn.as_str().to_string()),
    }
}

fn oxigraph_object_to_r5term(o: oxigraph::model::Term) -> R5Term {
    match o {
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
                R5Term::Literal {
                    lex,
                    dt: Some(lit.datatype().as_str().to_string()),
                    lang: None,
                }
            }
        }
    }
}

pub struct PersistentGraphIO {
    store: Store,
    offline: bool,
    strict: bool,
    store_path: PathBuf,
    r5_file: Option<R5tuFile>,
    r5_index: HashMap<String, R5GraphInfo>,
    loaded_graphs: Mutex<HashSet<String>>,
    dirty: bool,
    batch_depth: usize,
}

/// Predicates whose transitive closure the query layer precomputes by default
/// (in memory, on demand) for SPARQL `P+`/`P*` property paths. Covers the most
/// common property-path use cases on ontologies.
pub const DEFAULT_CLOSURE_PREDICATES: &[&str] = &[
    "http://www.w3.org/2000/01/rdf-schema#subClassOf",
    "http://www.w3.org/2000/01/rdf-schema#subPropertyOf",
    "http://www.w3.org/2002/07/owl#sameAs",
];

impl PersistentGraphIO {
    pub fn new(path: PathBuf, offline: bool, strict: bool) -> Result<Self> {
        // Locking is owned by the environment/catalog layer so custom and
        // built-in graph backends share identical concurrency semantics.
        std::fs::create_dir_all(&path)?;
        // On-disk file is an RDF5D `.r5tu` file; in-memory store is Oxigraph
        let store_path = path.join("store.r5tu");
        let store = Store::new()?;
        // Load RDF5D header/index for lazy graph loading.
        let (r5_file, r5_index) = if store_path.exists() {
            let file = R5tuFile::open(&store_path)?;
            let mut index = HashMap::new();
            for gr in file.enumerate_all()? {
                index.insert(
                    gr.graphname.clone(),
                    R5GraphInfo {
                        gid: gr.gid,
                        id: gr.id,
                        n_triples: gr.n_triples,
                    },
                );
            }
            (Some(file), index)
        } else {
            (None, HashMap::new())
        };

        Ok(Self {
            store,
            offline,
            strict,
            store_path,
            r5_file,
            r5_index,
            loaded_graphs: Mutex::new(HashSet::new()),
            dirty: false,
            batch_depth: 0,
        })
    }

    fn ensure_graph_loaded(&self, graphname: &str) -> Result<()> {
        // Lazy-load graphs from RDF5D into the in-memory store on first access.
        let mut loaded = self
            .loaded_graphs
            .lock()
            .map_err(|_| anyhow!("Failed to lock graph load state"))?;
        if loaded.contains(graphname) {
            return Ok(());
        }
        let graphname_str = graphname.to_string();
        let Some(info) = self.r5_index.get(graphname) else {
            return Ok(());
        };
        let Some(file) = self.r5_file.as_ref() else {
            return Ok(());
        };
        // Convert RDF5D triples into quads for Oxigraph.
        let gnn = NamedNode::new(graphname)
            .map_err(|e| anyhow!("Invalid graph name IRI in RDF5D: {}", e))?;
        let graphname = GraphName::NamedNode(gnn);
        let triples = file.oxigraph_triples(info.gid)?;
        let mut loader = self.store.bulk_loader();
        let mut quads_buf: Vec<Quad> = Vec::with_capacity(info.n_triples as usize);
        for res in triples {
            let t = res.map_err(|e| anyhow!("RDF5D read error: {}", e))?;
            quads_buf.push(Quad::new(
                t.subject,
                t.predicate,
                t.object,
                graphname.clone(),
            ));
        }
        // Commit as a single batch for better performance.
        loader.load_quads(quads_buf)?;
        loader.commit()?;
        loaded.insert(graphname_str);
        Ok(())
    }

    fn count_graph_triples(&self, graphname: &GraphName) -> Result<usize> {
        let mut count = 0usize;
        for quad in self
            .store
            .quads_for_pattern(None, None, None, Some(graphname.as_ref()))
        {
            quad?;
            count = count.saturating_add(1);
        }
        Ok(count)
    }

    fn update_index_for_graph(&mut self, graphname: &GraphName) -> Result<()> {
        let graphname_str = match graphname {
            GraphName::NamedNode(nn) => nn.as_str().to_string(),
            _ => return Err(anyhow!("Only named graphs are supported in RDF5D backend")),
        };
        let n_triples = self.count_graph_triples(graphname)?;
        let entry = self
            .r5_index
            .entry(graphname_str.clone())
            .or_insert(R5GraphInfo {
                gid: 0,
                id: graphname_str.clone(),
                n_triples: 0,
            });
        entry.n_triples = n_triples as u64;
        if entry.id.is_empty() {
            entry.id = graphname_str;
        }
        Ok(())
    }

    fn write_store_to_r5tu(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        // Serialize the in-memory dataset to RDF5D on disk, preserving graph boundaries.
        // Stream out all quads in the in-memory store to an RDF5D file atomically
        let opts = WriterOptions {
            zstd: true,
            with_crc: true,
        };
        let mut writer = StreamingWriter::new(&self.store_path, opts);

        let mut written_graphs = HashSet::new();
        let iter = self.store.quads_for_pattern(None, None, None, None);
        for q in iter {
            let q = q?;
            let gname_str = match q.graph_name {
                oxigraph::model::GraphName::NamedNode(ref nn) => nn.as_str().to_string(),
                _ => return Err(anyhow!("Only named graphs are supported in RDF5D backend")),
            };
            let id_str = gname_str.clone();
            written_graphs.insert(gname_str.clone());

            writer.add(Quint {
                id: id_str,
                s: oxigraph_subject_to_r5term(q.subject),
                p: R5Term::Iri(q.predicate.as_str().to_string()),
                o: oxigraph_object_to_r5term(q.object),
                gname: gname_str,
            })?;
        }

        // Copy any untouched graphs from the existing RDF5D file.
        if let Some(file) = self.r5_file.as_ref() {
            for (graphname, info) in &self.r5_index {
                if written_graphs.contains(graphname) {
                    continue;
                }
                let triples = file.oxigraph_triples(info.gid)?;
                for res in triples {
                    let t = res.map_err(|e| anyhow!("RDF5D read error: {}", e))?;
                    let gname_str = graphname.clone();
                    let id_str = if info.id.is_empty() {
                        gname_str.clone()
                    } else {
                        info.id.clone()
                    };
                    writer.add(Quint {
                        id: id_str,
                        s: oxigraph_subject_to_r5term(t.subject),
                        p: R5Term::Iri(t.predicate.as_str().to_string()),
                        o: oxigraph_object_to_r5term(t.object),
                        gname: gname_str,
                    })?;
                }
            }
        }

        // Finalize writes and mark the store clean.
        writer.finalize()?;
        self.dirty = false;
        // Query indexes are now built in memory on demand, so no sidecar is
        // written. Remove any sidecar left behind by an older version to
        // reclaim disk and avoid confusion.
        let _ = std::fs::remove_file(legacy_sidecar_path(&self.store_path));
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

    fn store_state(&self) -> Result<Option<BackendState>> {
        Ok(Some(file_backend_state(&self.store_path)?))
    }

    fn graph_ids(&self) -> Result<Vec<GraphIdentifier>> {
        // Persistent graphs are loaded lazily, so the in-memory Oxigraph store
        // is not authoritative until a graph is first read. Use the RDF5D
        // directory populated at open time instead.
        self.r5_index
            .keys()
            .map(|id| {
                NamedNode::new(id)
                    .map(|name| GraphIdentifier::new(name.as_ref()))
                    .map_err(|error| anyhow!(error.to_string()))
            })
            .collect()
    }

    fn add_named_graph(&mut self, id: GraphIdentifier, graph: Graph) -> Result<()> {
        let graphname = id.graphname()?;
        self.store.remove_named_graph(id.name())?;
        let mut loader = self.store.bulk_loader();
        loader.load_quads(
            graph
                .iter()
                .map(|t| Quad::new(t.subject, t.predicate, t.object, graphname.clone())),
        )?;
        loader.commit()?;
        self.update_index_for_graph(&graphname)?;
        let mut loaded = self
            .loaded_graphs
            .lock()
            .map_err(|_| anyhow!("Failed to lock graph load state"))?;
        if let GraphName::NamedNode(nn) = graphname {
            loaded.insert(nn.as_str().to_string());
        }
        drop(loaded);
        self.on_store_mutated()?;
        Ok(())
    }

    fn add(&mut self, location: OntologyLocation, overwrite: Overwrite) -> Result<Ontology> {
        let ont =
            add_ontology_to_store(&self.store, location, overwrite, self.offline, self.strict)?;
        let graphname = ont.id().graphname()?;
        self.update_index_for_graph(&graphname)?;
        let mut loaded = self
            .loaded_graphs
            .lock()
            .map_err(|_| anyhow!("Failed to lock graph load state"))?;
        if let GraphName::NamedNode(nn) = graphname {
            loaded.insert(nn.as_str().to_string());
        }
        drop(loaded);
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
        let graphname = ont.id().graphname()?;
        self.update_index_for_graph(&graphname)?;
        let mut loaded = self
            .loaded_graphs
            .lock()
            .map_err(|_| anyhow!("Failed to lock graph load state"))?;
        if let GraphName::NamedNode(nn) = graphname {
            loaded.insert(nn.as_str().to_string());
        }
        drop(loaded);
        self.on_store_mutated()?;
        Ok(ont)
    }

    fn remove(&mut self, id: &GraphIdentifier) -> Result<()> {
        let graphname = id.name();
        self.store.remove_named_graph(graphname)?;
        let graphname_str = graphname.as_str().to_string();
        self.r5_index.remove(&graphname_str);
        let mut loaded = self
            .loaded_graphs
            .lock()
            .map_err(|_| anyhow!("Failed to lock graph load state"))?;
        loaded.remove(&graphname_str);
        drop(loaded);
        self.on_store_mutated()?;
        Ok(())
    }

    fn ensure_loaded(&self, id: &GraphIdentifier) -> Result<()> {
        self.ensure_graph_loaded(id.name().as_str())
    }

    fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        let graphname = id.name().as_str();
        self.ensure_graph_loaded(graphname)?;
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
        let num_graphs = self.r5_index.len();
        let num_triples: usize = self.r5_index.values().map(|gr| gr.n_triples as usize).sum();
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
    r5_file: Option<R5tuFile>,
    r5_index: HashMap<String, R5GraphInfo>,
    loaded_graphs: Mutex<HashSet<String>>,
}

impl ReadOnlyPersistentGraphIO {
    pub fn new(path: PathBuf, offline: bool) -> Result<Self> {
        let store_path = path.join("store.r5tu");
        let store = Store::new()?;
        let (r5_file, r5_index) = if store_path.exists() {
            let file = R5tuFile::open(&store_path)?;
            let mut index = HashMap::new();
            for graph in file.enumerate_all()? {
                index.insert(
                    graph.graphname.clone(),
                    R5GraphInfo {
                        gid: graph.gid,
                        id: graph.id,
                        n_triples: graph.n_triples,
                    },
                );
            }
            (Some(file), index)
        } else {
            (None, HashMap::new())
        };
        Ok(Self {
            store,
            offline,
            store_path,
            r5_file,
            r5_index,
            loaded_graphs: Mutex::new(HashSet::new()),
        })
    }

    fn ensure_graph_loaded(&self, graphname: &str) -> Result<()> {
        let mut loaded = self
            .loaded_graphs
            .lock()
            .map_err(|_| anyhow!("Failed to lock graph load state"))?;
        if loaded.contains(graphname) {
            return Ok(());
        }
        let Some(info) = self.r5_index.get(graphname) else {
            return Ok(());
        };
        let Some(file) = self.r5_file.as_ref() else {
            return Ok(());
        };
        let graph_name = GraphName::NamedNode(
            NamedNode::new(graphname)
                .map_err(|error| anyhow!("Invalid graph name IRI in RDF5D: {error}"))?,
        );
        let triples = file.oxigraph_triples(info.gid)?;
        let mut loader = self.store.bulk_loader();
        let mut quads = Vec::with_capacity(info.n_triples as usize);
        for triple in triples {
            let triple = triple.map_err(|error| anyhow!("RDF5D read error: {error}"))?;
            quads.push(Quad::new(
                triple.subject,
                triple.predicate,
                triple.object,
                graph_name.clone(),
            ));
        }
        loader.load_quads(quads)?;
        loader.commit()?;
        loaded.insert(graphname.to_string());
        Ok(())
    }
}

impl Drop for PersistentGraphIO {
    fn drop(&mut self) {
        if self.dirty {
            if let Err(err) = self.write_store_to_r5tu() {
                error!("Failed to flush RDF5D store on drop: {err}");
            }
        }
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

    fn store_state(&self) -> Result<Option<BackendState>> {
        Ok(Some(file_backend_state(&self.store_path)?))
    }

    fn graph_ids(&self) -> Result<Vec<GraphIdentifier>> {
        self.r5_index
            .keys()
            .map(|id| {
                NamedNode::new(id)
                    .map(|name| GraphIdentifier::new(name.as_ref()))
                    .map_err(|error| anyhow!(error.to_string()))
            })
            .collect()
    }

    fn ensure_loaded(&self, id: &GraphIdentifier) -> Result<()> {
        self.ensure_graph_loaded(id.name().as_str())
    }

    fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        self.ensure_graph_loaded(id.name().as_str())?;
        let mut graph = Graph::new();
        for quad in
            self.store
                .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(id.name())))
        {
            graph.insert(quad?.as_ref());
        }
        Ok(graph)
    }

    fn add_named_graph(&mut self, _id: GraphIdentifier, _graph: Graph) -> Result<()> {
        Err(anyhow!("Cannot add to read-only store"))
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
        // Wrap an externally-managed Store without taking ownership of its path.
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
        // Build an in-memory store for tests and ephemeral usage.
        Ok(Self {
            store: Store::new()?,
            offline,
            strict,
        })
    }

    pub fn add_graph(&mut self, id: GraphIdentifier, graph: Graph) -> Result<()> {
        // Replace any existing named graph with the provided graph data.
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
