//! Managed storage adapter: one committed selection and one complete candidate.
//! The candidate owns catalog, configuration and staged graphs together. Only
//! necessary nested checkpoints clone metadata; staged buffers remain shared.

use crate::catalog::BackendState;
use crate::config::Config;
use crate::environment::Environment;
use crate::io::{fetch_source, parse_ontology_source, GraphIO, ParsedOntology, StoreStats};
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use crate::options::Overwrite;
use crate::snapshots::{EnvironmentSnapshot, SnapshotCandidate, SnapshotReader, SnapshotWriter};
use crate::staged_graph::StagedGraph;
use crate::FailedImport;
use anyhow::{bail, Result};
use oxigraph::io::RdfFormat;
use oxigraph::model::{Dataset, Graph, QuadRef};
use std::path::{Path, PathBuf};
use std::sync::Arc;

enum Connection {
    Reader(SnapshotReader),
    Writer(SnapshotWriter),
}

impl Connection {
    fn selected(&self) -> &EnvironmentSnapshot {
        match self {
            Self::Reader(reader) => reader.selected(),
            Self::Writer(writer) => writer.selected(),
        }
    }
    fn snapshot(&self) -> Arc<EnvironmentSnapshot> {
        match self {
            Self::Reader(reader) => reader.snapshot(),
            Self::Writer(writer) => writer.snapshot(),
        }
    }
}

pub(crate) struct SnapshotGraphIO {
    connection: Connection,
    candidate: Option<SnapshotCandidate>,
    checkpoints: Vec<Option<SnapshotCandidate>>,
    // Read-only overrides are session settings, never a writable candidate.
    reader_config: Option<Config>,
    directory: PathBuf,
}

impl SnapshotGraphIO {
    pub(crate) fn from_writer(writer: SnapshotWriter) -> Self {
        Self::from_connection(Connection::Writer(writer))
    }
    pub(crate) fn create(config: Config) -> Result<Self> {
        Ok(Self::from_writer(SnapshotWriter::create(config)?))
    }
    pub(crate) fn open(root: &Path, read_only: bool) -> Result<Self> {
        Ok(Self::from_connection(if read_only {
            Connection::Reader(SnapshotReader::open(root)?)
        } else {
            Connection::Writer(SnapshotWriter::open(root)?)
        }))
    }
    fn from_connection(connection: Connection) -> Self {
        let directory = connection
            .selected()
            .config()
            .root
            .join(".ontoenv-snapshots");
        Self {
            connection,
            candidate: None,
            checkpoints: Vec::new(),
            reader_config: None,
            directory,
        }
    }
    pub(crate) fn environment(&self) -> &Environment {
        self.candidate.as_ref().map_or_else(
            || self.connection.selected().environment(),
            |candidate| &candidate.environment,
        )
    }
    pub(crate) fn config(&self) -> &Config {
        if let Some(config) = &self.reader_config {
            return config;
        }
        self.candidate.as_ref().map_or_else(
            || self.connection.selected().config(),
            |candidate| &candidate.config,
        )
    }
    pub(crate) fn candidate_mut(&mut self) -> &mut SnapshotCandidate {
        self.candidate
            .get_or_insert_with(|| SnapshotCandidate::from_snapshot(self.connection.selected()))
    }
    pub(crate) fn config_mut(&mut self) -> &mut Config {
        if matches!(self.connection, Connection::Reader(_)) {
            self.reader_config
                .get_or_insert_with(|| self.connection.selected().config().clone())
        } else {
            &mut self.candidate_mut().config
        }
    }
    fn writable(&self) -> Result<()> {
        if matches!(self.connection, Connection::Reader(_)) {
            bail!("Cannot mutate a read-only environment");
        }
        Ok(())
    }
    fn put(&mut self, parsed: ParsedOntology, overwrite: Overwrite) -> Result<Ontology> {
        self.writable()?;
        if !overwrite.as_bool() {
            if let Some(ontology) = self
                .environment()
                .get_ontology_by_name(parsed.ontology.name().as_ref())
            {
                return Ok(ontology.clone());
            }
        }
        let ParsedOntology {
            ontology,
            triples,
            bytes,
            ..
        } = parsed;
        drop(bytes);
        self.candidate_mut().changes.insert(
            ontology.name().as_str().to_owned(),
            Some(Arc::new(StagedGraph::from_triples(triples))),
        );
        Ok(ontology)
    }
    fn publish_candidate(&mut self) -> Result<()> {
        let Some(candidate) = self.candidate.take() else {
            return Ok(());
        };
        let base = self.connection.selected();
        if candidate.changes.is_empty()
            && serde_json::to_value(&candidate.environment)?
                == serde_json::to_value(base.environment())?
            && serde_json::to_value(&candidate.config)? == serde_json::to_value(base.config())?
        {
            return Ok(());
        }
        match &mut self.connection {
            Connection::Writer(writer) => {
                writer.publish_candidate(candidate)?;
            }
            Connection::Reader(_) => bail!("Cannot publish a read-only environment"),
        }
        Ok(())
    }
    pub(crate) fn publish(&mut self) -> Result<()> {
        if matches!(self.connection, Connection::Writer(_)) && self.checkpoints.is_empty() {
            self.publish_candidate()?;
        }
        Ok(())
    }
}

impl SnapshotGraphIO {
    pub(crate) fn prune(&mut self) -> Result<crate::snapshots::PruneReport> {
        if self.candidate.is_some() || !self.checkpoints.is_empty() {
            bail!("Cannot prune with staged changes");
        }
        match &mut self.connection {
            Connection::Writer(writer) => writer.prune(),
            Connection::Reader(_) => bail!("Cleanup requires a snapshot writer"),
        }
    }
    pub(crate) fn snapshot(&self) -> Arc<EnvironmentSnapshot> {
        self.connection.snapshot()
    }
    pub(crate) fn refresh(&mut self) -> Result<bool> {
        if self.candidate.is_some() || !self.checkpoints.is_empty() {
            bail!("Cannot refresh staged changes");
        }
        match &mut self.connection {
            Connection::Reader(reader) => {
                let changed = reader.refresh()?;
                if changed {
                    self.reader_config = None;
                }
                Ok(changed)
            }
            Connection::Writer(_) => Ok(false),
        }
    }
}

impl GraphIO for SnapshotGraphIO {
    fn is_offline(&self) -> bool {
        self.config().offline
    }
    fn io_type(&self) -> String {
        if matches!(self.connection, Connection::Reader(_)) {
            "read-only"
        } else {
            "persistent"
        }
        .into()
    }
    fn store_location(&self) -> Option<&Path> {
        Some(&self.directory)
    }
    // All durable publication and flushing occurs at the candidate boundary.
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
    fn store_state(&self) -> Result<Option<BackendState>> {
        let info = self.connection.selected().info();
        Ok(Some(BackendState {
            id: info.environment_id.clone(),
            revision: info.generation.to_string(),
        }))
    }
    fn graph_ids(&self) -> Result<Vec<GraphIdentifier>> {
        Ok(self.environment().ontologies().keys().cloned().collect())
    }
    fn add(&mut self, location: OntologyLocation, overwrite: Overwrite) -> Result<Ontology> {
        let (bytes, format) = fetch_source(&location, self.config().offline)?;
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
            parse_ontology_source(
                location,
                bytes,
                format,
                self.config().require_ontology_names,
            )?,
            overwrite,
        )
    }
    fn add_parsed(&mut self, parsed: ParsedOntology, overwrite: Overwrite) -> Result<Ontology> {
        self.put(parsed, overwrite)
    }
    fn add_named_graph(&mut self, id: GraphIdentifier, graph: Graph) -> Result<()> {
        self.writable()?;
        self.candidate_mut().changes.insert(
            id.name().as_str().into(),
            Some(Arc::new(StagedGraph::Indexed(Box::new(graph)))),
        );
        Ok(())
    }
    fn remove(&mut self, id: &GraphIdentifier) -> Result<()> {
        self.writable()?;
        self.candidate_mut()
            .changes
            .insert(id.name().as_str().into(), None);
        Ok(())
    }
    fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        match self
            .candidate
            .as_ref()
            .and_then(|candidate| candidate.changes.get(id.name().as_str()))
        {
            Some(Some(graph)) => Ok(graph.to_graph()),
            Some(None) => Ok(Graph::new()),
            None => {
                let snapshot = self.connection.selected();
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
        let snapshot = self.connection.selected();
        let num_triples = match &self.candidate {
            None => snapshot.rdf().triple_count(rdf5d::Scope::All)?,
            Some(candidate) => {
                let mut count = 0;
                for id in candidate.environment.ontologies().keys() {
                    count += match candidate.changes.get(id.name().as_str()) {
                        Some(Some(graph)) => graph.iter().count(),
                        Some(None) => 0,
                        None => snapshot
                            .rdf()
                            .triple_count(rdf5d::Scope::ByName(id.name().as_str()))?,
                    };
                }
                count
            }
        };
        Ok(StoreStats {
            num_graphs: self.environment().ontologies().len(),
            num_triples,
        })
    }
    fn begin_batch(&mut self) -> Result<()> {
        self.writable()?;
        self.checkpoints.push(self.candidate.clone());
        Ok(())
    }
    fn end_batch(&mut self) -> Result<()> {
        // Keep the outer checkpoint through publication. A failed validation or
        // uncertain publication still restores the complete pre-operation candidate.
        if self.checkpoints.len() == 1 {
            self.publish_candidate()?;
        }
        self.checkpoints.pop();
        Ok(())
    }
    fn abort_batch(&mut self) -> Result<()> {
        self.candidate = self.checkpoints.pop().unwrap_or_default();
        Ok(())
    }
}
