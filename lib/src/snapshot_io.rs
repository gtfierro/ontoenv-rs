//! GraphIO adapter for the normal API: graph writes are private until the
//! complete Environment/Config is published, and nested failures roll back.

use crate::catalog::BackendState;
use crate::config::Config;
use crate::environment::Environment;
use crate::io::{fetch_source, parse_ontology_source, GraphIO, ParsedOntology, StoreStats};
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use crate::options::Overwrite;
use crate::snapshots::{EnvironmentSnapshot, SnapshotReader, SnapshotWriter};
use crate::staged_graph::StagedGraph;
use crate::FailedImport;
use anyhow::{bail, Result};
use oxigraph::io::RdfFormat;
use oxigraph::model::{Dataset, Graph, QuadRef};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct PendingGraph {
    id: GraphIdentifier,
    graph: Arc<StagedGraph>,
    ontology: Option<Ontology>,
}

type Overlay = BTreeMap<String, Option<PendingGraph>>;

enum Connection {
    Reader(SnapshotReader),
    Writer(SnapshotWriter),
}

impl Connection {
    fn snapshot(&self) -> Arc<EnvironmentSnapshot> {
        match self {
            Self::Reader(reader) => reader.snapshot(),
            Self::Writer(writer) => writer.snapshot(),
        }
    }
}

struct State {
    connection: Connection,
    pending: Overlay,
    checkpoints: Vec<Overlay>,
}

pub(crate) struct SnapshotGraphIO {
    state: Mutex<State>,
    directory: PathBuf,
    offline: bool,
    require_names: bool,
}

impl SnapshotGraphIO {
    pub(crate) fn from_writer(writer: SnapshotWriter) -> Self {
        Self::from_connection(Connection::Writer(writer))
    }
    pub(crate) fn create(config: Config) -> Result<Self> {
        Ok(Self::from_connection(Connection::Writer(
            SnapshotWriter::create(config)?,
        )))
    }
    pub(crate) fn open(root: &Path, read_only: bool) -> Result<Self> {
        let connection = if read_only {
            Connection::Reader(SnapshotReader::open(root)?)
        } else {
            Connection::Writer(SnapshotWriter::open(root)?)
        };
        Ok(Self::from_connection(connection))
    }
    fn from_connection(connection: Connection) -> Self {
        let snapshot = connection.snapshot();
        Self {
            offline: snapshot.config().offline,
            require_names: snapshot.config().require_ontology_names,
            directory: snapshot.config().root.join(".ontoenv-snapshots"),
            state: Mutex::new(State {
                connection,
                pending: BTreeMap::new(),
                checkpoints: Vec::new(),
            }),
        }
    }
    fn writable(&self) -> Result<()> {
        if matches!(self.state.lock().unwrap().connection, Connection::Reader(_)) {
            bail!("Cannot mutate a read-only environment");
        }
        Ok(())
    }
    fn put(&mut self, parsed: ParsedOntology, overwrite: Overwrite) -> Result<Ontology> {
        self.writable()?;
        let name = parsed.ontology.name().as_str().to_owned();
        let mut state = self.state.lock().unwrap();
        if !overwrite.as_bool() {
            match state.pending.get(&name) {
                Some(Some(pending)) => {
                    if let Some(ontology) = &pending.ontology {
                        return Ok(ontology.clone());
                    }
                    return match &*pending.graph {
                        StagedGraph::Triples(triples) => Ontology::from_triples(
                            triples,
                            pending.id.location().clone(),
                            self.require_names,
                        ),
                        StagedGraph::Indexed(graph) => Ontology::from_graph(
                            graph,
                            pending.id.location().clone(),
                            self.require_names,
                        ),
                    };
                }
                Some(None) => {}
                None => {
                    if let Some(ontology) = state
                        .connection
                        .snapshot()
                        .environment()
                        .get_ontology_by_name(parsed.ontology.name().as_ref())
                    {
                        return Ok(ontology.clone());
                    }
                }
            }
        }
        let ParsedOntology {
            ontology,
            triples,
            bytes,
            ..
        } = parsed;
        drop(bytes);
        state.pending.insert(
            name,
            Some(PendingGraph {
                id: ontology.id().clone(),
                graph: Arc::new(StagedGraph::from_triples(triples)),
                ontology: Some(ontology.clone()),
            }),
        );
        Ok(ontology)
    }
}

impl GraphIO for SnapshotGraphIO {
    fn prune_snapshots(&mut self) -> Result<crate::snapshots::PruneReport> {
        let mut state = self.state.lock().unwrap();
        if !state.pending.is_empty() || !state.checkpoints.is_empty() {
            bail!("Cannot prune with staged changes");
        }
        match &mut state.connection {
            Connection::Writer(writer) => writer.prune(),
            Connection::Reader(_) => bail!("Cleanup requires a snapshot writer"),
        }
    }
    fn environment_snapshot(&self) -> Option<Arc<EnvironmentSnapshot>> {
        Some(self.state.lock().unwrap().connection.snapshot())
    }
    fn publish_environment(&self, environment: &Environment, config: &Config) -> Result<bool> {
        let mut state = self.state.lock().unwrap();
        if matches!(state.connection, Connection::Reader(_)) {
            return Ok(true);
        }
        if !state.checkpoints.is_empty() {
            return Ok(true);
        }
        let base = state.connection.snapshot();
        if state.pending.is_empty()
            && serde_json::to_value(environment)? == serde_json::to_value(base.environment())?
            && serde_json::to_value(config)? == serde_json::to_value(base.config())?
        {
            return Ok(true);
        }
        let changes = state
            .pending
            .iter()
            .map(|(name, pending)| {
                (
                    name.clone(),
                    pending.as_ref().map(|pending| Arc::clone(&pending.graph)),
                )
            })
            .collect();
        if let Connection::Writer(writer) = &mut state.connection {
            writer.publish_state(environment.clone(), config.clone(), changes)?;
        }
        state.pending.clear();
        Ok(true)
    }
    fn refresh_environment(&mut self) -> Result<bool> {
        let mut state = self.state.lock().unwrap();
        if !state.pending.is_empty() || !state.checkpoints.is_empty() {
            bail!("Cannot refresh staged changes");
        }
        match &mut state.connection {
            Connection::Reader(reader) => reader.refresh(),
            Connection::Writer(_) => Ok(false),
        }
    }
    fn is_offline(&self) -> bool {
        self.offline
    }
    fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }
    fn set_require_ontology_names(&mut self, require: bool) {
        self.require_names = require;
    }
    fn io_type(&self) -> String {
        if matches!(self.state.lock().unwrap().connection, Connection::Reader(_)) {
            "read-only"
        } else {
            "persistent"
        }
        .into()
    }
    fn store_location(&self) -> Option<&Path> {
        Some(&self.directory)
    }
    // Publication is handled by publish_environment, including its durability
    // barriers. There is no separate Oxigraph store to flush, even with rocksdb.
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
    fn store_state(&self) -> Result<Option<BackendState>> {
        let snapshot = self.environment_snapshot().unwrap();
        Ok(Some(BackendState {
            id: snapshot.info().environment_id.clone(),
            revision: snapshot.info().generation.to_string(),
        }))
    }
    fn graph_ids(&self) -> Result<Vec<GraphIdentifier>> {
        let state = self.state.lock().unwrap();
        let snapshot = state.connection.snapshot();
        let mut ids: BTreeMap<_, _> = snapshot
            .ontologies()
            .keys()
            .map(|id| (id.name().as_str().to_owned(), id.clone()))
            .collect();
        for (name, graph) in &state.pending {
            match graph {
                Some(pending) => {
                    ids.insert(name.clone(), pending.id.clone());
                }
                None => {
                    ids.remove(name);
                }
            }
        }
        Ok(ids.into_values().collect())
    }
    fn add(&mut self, location: OntologyLocation, overwrite: Overwrite) -> Result<Ontology> {
        let (bytes, format) = fetch_source(&location, self.offline)?;
        self.add_from_bytes(location, bytes, format, overwrite)
    }
    fn add_from_bytes(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
        overwrite: Overwrite,
    ) -> Result<Ontology> {
        self.put(
            parse_ontology_source(location, bytes, format, self.require_names)?,
            overwrite,
        )
    }
    fn add_parsed(&mut self, parsed: ParsedOntology, overwrite: Overwrite) -> Result<Ontology> {
        self.put(parsed, overwrite)
    }
    fn add_named_graph(&mut self, id: GraphIdentifier, graph: Graph) -> Result<()> {
        self.writable()?;
        self.state.lock().unwrap().pending.insert(
            id.name().as_str().into(),
            Some(PendingGraph {
                id,
                graph: Arc::new(StagedGraph::Indexed(Box::new(graph))),
                ontology: None,
            }),
        );
        Ok(())
    }
    fn remove(&mut self, id: &GraphIdentifier) -> Result<()> {
        self.writable()?;
        self.state
            .lock()
            .unwrap()
            .pending
            .insert(id.name().as_str().into(), None);
        Ok(())
    }
    fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        let state = self.state.lock().unwrap();
        match state.pending.get(id.name().as_str()) {
            Some(Some(pending)) => Ok(pending.graph.to_graph()),
            Some(None) => Ok(Graph::new()),
            None => {
                let snapshot = state.connection.snapshot();
                let Some(id) = snapshot.resolve(id.name()) else {
                    return Ok(Graph::new());
                };
                snapshot.get_graph(&id)
            }
        }
    }
    fn union_graph(&self, ids: &[GraphIdentifier]) -> (Dataset, Vec<FailedImport>) {
        let mut dataset = Dataset::new();
        let mut errors = Vec::new();
        for id in ids {
            match self.get_graph(id) {
                Ok(graph) => {
                    for triple in graph.iter() {
                        dataset.insert(QuadRef::new(
                            triple.subject,
                            triple.predicate,
                            triple.object,
                            id.name(),
                        ));
                    }
                }
                Err(error) => errors.push(FailedImport::new(id.clone(), error.to_string())),
            }
        }
        (dataset, errors)
    }
    fn size(&self) -> Result<StoreStats> {
        let snapshot = self.environment_snapshot().unwrap();
        Ok(StoreStats {
            num_graphs: snapshot.ontologies().len(),
            num_triples: snapshot.rdf().triple_count(rdf5d::Scope::All)?,
        })
    }
    fn begin_batch(&mut self) -> Result<()> {
        self.writable()?;
        let mut state = self.state.lock().unwrap();
        let checkpoint = state.pending.clone();
        state.checkpoints.push(checkpoint);
        Ok(())
    }
    fn end_batch(&mut self) -> Result<()> {
        self.state.lock().unwrap().checkpoints.pop();
        Ok(())
    }
    fn abort_batch(&mut self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.pending = state.checkpoints.pop().unwrap_or_default();
        Ok(())
    }
}
