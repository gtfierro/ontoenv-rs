//! Atomic publication of OntoEnv versions backed by immutable RDF5D files.
//!
//! Default managed storage uses `.ontoenv-snapshots`. Legacy stores require
//! explicit migration, which retains the original files.
//! Graph changes are staged over an immutable base. An atomically replaced
//! `CURRENT` file selects an immutable manifest bundling graph references,
//! metadata, aliases and configuration. No database or reader recovery is needed.
//! Owned readers pin their backing files across publication and writer restarts.

use crate::api::ImportPaths;
use crate::catalog_queries::{self, CatalogIndex};
use crate::config::Config;
use crate::environment::Environment;
use crate::io::{
    fetch_source, oxigraph_object_to_r5term, oxigraph_subject_to_r5term, parse_ontology_source,
};
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use crate::staged_graph::StagedGraph;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use oxigraph::io::RdfFormat;
use oxigraph::model::{Dataset, Graph, GraphNameRef, NamedNode, NamedNodeRef, QuadRef};
use rdf5d::{Quint, Snapshot, StreamingWriter, WriterOptions};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

const DIRECTORY: &str = ".ontoenv-snapshots";
const SCHEMA_VERSION: u32 = 3;
const CURRENT: &str = "CURRENT";

#[path = "snapshot_retention.rs"]
mod retention;
pub use retention::PruneReport;

pub(crate) fn read_config(root: &Path) -> Result<Option<Config>> {
    let directory = root.join(DIRECTORY);
    if !directory.exists() {
        return Ok(None);
    }
    Ok(Some(read_head(&directory)?.config))
}

#[derive(Debug, Serialize, Deserialize)]
struct Head {
    schema_version: u32,
    manifest: String,
    manifest_hash: String,
    #[serde(default)]
    previous: Option<ManifestRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestRef {
    manifest: String,
    manifest_hash: String,
}

impl Head {
    fn selected(&self) -> ManifestRef {
        ManifestRef {
            manifest: self.manifest.clone(),
            manifest_hash: self.manifest_hash.clone(),
        }
    }
}

/// Stable identity and timing of one committed environment generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Identity of the environment, independent of its filesystem pathname.
    pub environment_id: String,
    /// Monotonic committed generation (the initial empty generation is zero).
    pub generation: u64,
    /// Previous committed generation, absent for the initial generation.
    pub parent_generation: Option<u64>,
    /// Time recorded while preparing the commit; ordering uses `generation`.
    pub committed_at: DateTime<Utc>,
    /// Time this process opened the immutable generation.
    pub opened_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Generation {
    schema_version: u32,
    info: SnapshotInfo,
    object_id: String,
    object_length: u64,
    environment: Environment,
    config: Config,
}

/// A coherent immutable catalog, configuration and RDF5D query snapshot.
///
/// Clones of its owning `Arc` keep graphs and query indexes alive across refresh
/// and writer commits. There is no mutation or persistence API on this type.
#[derive(Debug)]
pub struct EnvironmentSnapshot {
    info: SnapshotInfo,
    environment: Environment,
    config: Config,
    rdf: Arc<Snapshot>,
    object_id: String,
    object_length: u64,
    catalog_index: OnceLock<CatalogIndex>,
}

impl EnvironmentSnapshot {
    pub(crate) fn catalog_index(&self) -> &CatalogIndex {
        self.catalog_index
            .get_or_init(|| CatalogIndex::new(&self.environment))
    }

    /// Resolve a dependency closure from this catalog without fetching sources.
    /// Depth zero selects only the root; imports beyond the limit are not checked.
    pub fn get_closure(
        &self,
        id: &GraphIdentifier,
        recursion_depth: i32,
    ) -> Result<Vec<GraphIdentifier>> {
        self.catalog_index()
            .closure(id, recursion_depth, self.config.strict)
    }

    /// Direct importers, resolving aliases through this generation's catalog.
    pub fn get_importers(&self, name: &NamedNode) -> Result<Vec<GraphIdentifier>> {
        self.catalog_index().importers(name)
    }

    /// Explain resolved or missing imports using maximal paths without repeated nodes.
    pub fn explain_import(&self, name: &NamedNode) -> ImportPaths {
        self.catalog_index().explain_import(name)
    }

    /// Importer paths, ordered from the most distant importer to the target.
    pub fn get_import_paths(&self, name: &NamedNode) -> Result<Vec<Vec<GraphIdentifier>>> {
        match self.explain_import(name) {
            ImportPaths::Present(paths) => Ok(paths),
            ImportPaths::Missing { .. } => bail!("Ontology not found"),
        }
    }

    /// Merge graph namespaces in selection order; explicit bindings win.
    pub fn get_namespaces_for_graphs(
        &self,
        ids: &[GraphIdentifier],
    ) -> Result<HashMap<String, String>> {
        let ontologies = ids
            .iter()
            .map(|id| {
                self.ontologies()
                    .get(id)
                    .ok_or_else(|| anyhow::anyhow!("Ontology {} not found", id.name()))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(catalog_queries::namespaces(
            ontologies,
            &self.config.namespaces,
        ))
    }

    /// Merge namespaces in canonical IRI order, followed by explicit bindings.
    pub fn get_all_namespaces(&self) -> HashMap<String, String> {
        let mut ontologies: Vec<_> = self.ontologies().values().collect();
        ontologies.sort_by(|a, b| a.name().as_str().cmp(b.name().as_str()));
        catalog_queries::namespaces(ontologies, &self.config.namespaces)
    }

    /// Merge namespaces for one graph or its entire dependency closure.
    pub fn get_namespaces(
        &self,
        id: &GraphIdentifier,
        include_closure: bool,
    ) -> Result<HashMap<String, String>> {
        if include_closure {
            self.get_namespaces_for_graphs(&self.get_closure(id, -1)?)
        } else {
            self.get_namespaces_for_graphs(std::slice::from_ref(id))
        }
    }

    /// Materialize a dataset union, retaining canonical named-graph contexts.
    /// Repeated identifiers are idempotent. An empty selection yields an empty dataset.
    pub fn copy_dataset(&self, ids: &[GraphIdentifier]) -> Result<Dataset> {
        // Validate the whole selection before reading blocks, including empty graphs.
        self.view(ids)?;
        let mut dataset = Dataset::new();
        for id in ids.iter().collect::<HashSet<_>>() {
            let name = GraphNameRef::NamedNode(id.name());
            if let Some(gids) = self.rdf.gids_for_name(id.name().as_str()) {
                for &gid in gids {
                    for triple in self.rdf.file().oxigraph_triples(gid)? {
                        let triple = triple?;
                        dataset.insert(QuadRef::new(
                            &triple.subject,
                            &triple.predicate,
                            &triple.object,
                            name,
                        ));
                    }
                }
            }
        }
        Ok(dataset)
    }

    /// Materialize the graph projection of a union, deduplicating shared triples.
    pub fn copy_union(&self, ids: &[GraphIdentifier]) -> Result<Graph> {
        self.view(ids)?;
        let mut graph = Graph::new();
        for id in ids.iter().collect::<HashSet<_>>() {
            if let Some(gids) = self.rdf.gids_for_name(id.name().as_str()) {
                for &gid in gids {
                    for triple in self.rdf.file().oxigraph_triples(gid)? {
                        graph.insert(triple?.as_ref());
                    }
                }
            }
        }
        Ok(graph)
    }
    /// Information about this generation, rather than the current filesystem.
    pub fn info(&self) -> &SnapshotInfo {
        &self.info
    }

    /// Persisted configuration captured with this generation.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Ontology metadata, including aliases, captured with the graph contents.
    pub fn environment(&self) -> &Environment {
        &self.environment
    }

    /// All ontologies in this generation; this does not read graph blocks.
    pub fn ontologies(&self) -> &HashMap<GraphIdentifier, Ontology> {
        self.environment.ontologies()
    }

    /// Existing RDF5D query engine, retaining the whole-store term dictionary.
    /// Keep this `Arc` alive for the lifetime of any query view built from it.
    pub fn rdf(&self) -> Arc<Snapshot> {
        Arc::clone(&self.rdf)
    }

    /// Borrow a graph-scoped query view from this immutable environment version.
    /// Keep the view to reuse its lazy scoped indexes across queries. These are
    /// distinct from the whole-store indexes shared through [`Self::rdf`].
    pub fn view_graph(&self, id: &GraphIdentifier) -> Result<rdf5d::View<'_>> {
        self.view(std::slice::from_ref(id))
    }

    /// Borrow a scoped query view, rejecting identifiers outside this catalog.
    /// The Rust borrow keeps this version alive for the view's entire lifetime.
    /// Empty catalog graphs are valid scopes with no physical triples.
    pub fn view(&self, ids: &[GraphIdentifier]) -> Result<rdf5d::View<'_>> {
        let mut names = Vec::with_capacity(ids.len());
        for id in ids {
            if !self.ontologies().contains_key(id) {
                bail!(
                    "Graph {id} is not present in generation {}",
                    self.info.generation
                );
            }
            names.push(id.name().as_str());
        }
        Ok(rdf5d::View::from_names(&self.rdf, &names))
    }

    /// Copy one graph from this generation; empty and missing graphs differ.
    pub fn copy_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        if !self.environment.ontologies().contains_key(id) {
            bail!(
                "Graph {id} is not present in generation {}",
                self.info.generation
            );
        }
        let mut graph = Graph::new();
        if let Some(gids) = self.rdf.gids_for_name(id.name().as_str()) {
            for &gid in gids {
                for triple in self.rdf.file().oxigraph_triples(gid)? {
                    graph.insert(triple?.as_ref());
                }
            }
        }
        Ok(graph)
    }

    /// Resolve a declared name or explicit alias using this generation's catalog.
    pub fn resolve(&self, name: NamedNodeRef<'_>) -> Option<GraphIdentifier> {
        self.environment
            .get_ontology_by_name(name)
            .map(|ontology| ontology.id().clone())
    }
}

/// Read-only connection whose current snapshot changes only on explicit refresh.
pub struct SnapshotReader {
    directory: PathBuf,
    current: Arc<EnvironmentSnapshot>,
}

impl SnapshotReader {
    /// Open the committed head while a writer may remain open and commit.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let directory = fs::canonicalize(root.as_ref())?.join(DIRECTORY);
        let current = open_head(&directory)?;
        Ok(Self { directory, current })
    }

    /// Retain a snapshot independently of this reader and subsequent refreshes.
    pub fn snapshot(&self) -> Arc<EnvironmentSnapshot> {
        Arc::clone(&self.current)
    }

    pub(crate) fn selected(&self) -> &EnvironmentSnapshot {
        &self.current
    }

    /// Compare with committed head without changing this reader's snapshot.
    pub fn is_current(&self) -> Result<bool> {
        let head = read_head(&self.directory)?;
        Ok(head.info.environment_id == self.current.info.environment_id
            && head.info.generation == self.current.info.generation)
    }

    /// Validate and swap the latest snapshot as a unit. Errors preserve the old one.
    /// Returns whether the selected committed generation changed.
    pub fn refresh(&mut self) -> Result<bool> {
        let _gate = retention::read_gate(&self.directory)?;
        let generation = read_head_unlocked(&self.directory)?;
        if generation.info.environment_id == self.current.info.environment_id
            && generation.info.generation == self.current.info.generation
        {
            return Ok(false);
        }
        let next = if generation.info.environment_id == self.current.info.environment_id
            && generation.object_id == self.current.object_id
            && generation.object_length == self.current.object_length
        {
            // Metadata-only publication still selects the same immutable file.
            // Preserve its mapping and lazy whole-store indexes across refresh.
            validate_catalog(&generation, &self.current.rdf)?;
            snapshot_from_generation(generation, Arc::clone(&self.current.rdf))
        } else {
            open_generation(generation, &self.directory)?
        };
        self.current = next;
        Ok(true)
    }
}

/// The sole writer connection. Its stable lock never conflicts with readers.
///
/// Unlike legacy `OntoEnv`, dropping this handle never publishes graph changes.
pub struct SnapshotWriter {
    directory: PathBuf,
    current: Arc<EnvironmentSnapshot>,
    // A failed publication may already be durable. Never publish again from a
    // possibly stale base until a new writer has reconciled the manifest.
    needs_reopen: bool,
    // Keep the stable writer lock held for the lifetime of this handle.
    _writer_lock: WriterLock,
}

impl SnapshotWriter {
    /// Create an empty environment, refusing to recreate existing data.
    /// A legacy `.ontoenv` directory requires explicit migration.
    pub fn create(config: Config) -> Result<Self> {
        if config.root.join(".ontoenv").exists() {
            bail!("Legacy environment requires explicit migration: run `ontoenv migrate`");
        }
        Self::initialize(config, Environment::new(), None, || Ok(()))
    }

    fn initialize(
        mut config: Config,
        mut environment: Environment,
        source: Option<&Snapshot>,
        before_install: impl FnOnce() -> Result<()>,
    ) -> Result<Self> {
        if config.temporary || config.external_graph_store.is_some() {
            bail!("Snapshot storage requires persistent, managed graph storage");
        }
        config.build_globsets()?;
        config.build_ontology_regexes()?;
        fs::create_dir_all(&config.root)?;
        config.root = fs::canonicalize(&config.root)?;
        let writer_lock = acquire_writer_lock(&config.root)?;
        let directory = config.root.join(DIRECTORY);
        if directory.exists() {
            bail!(
                "Snapshot environment already exists at {}; open it instead",
                directory.display()
            );
        }
        // Prepare initialization outside the visible directory. A validation or
        // serialization error must not strand an unopenable environment there.
        let initialization = tempfile::Builder::new()
            .prefix(".ontoenv-snapshots-init-")
            .tempdir_in(&config.root)?;
        fs::create_dir(initialization.path().join("objects"))?;
        fs::create_dir(initialization.path().join("manifests"))?;
        retention::prepare_layout(initialization.path())?;
        environment.set_default_policy(&config.resolution_policy)?;
        let (object_id, object_length, rdf) =
            write_object(initialization.path(), source, &BTreeMap::new())?;
        let info = SnapshotInfo {
            environment_id: blake3::hash(&rand::random::<[u8; 32]>())
                .to_hex()
                .to_string(),
            generation: 0,
            parent_generation: None,
            committed_at: Utc::now(),
            opened_at: Utc::now(),
        };
        let generation = Generation {
            schema_version: SCHEMA_VERSION,
            info,
            object_id,
            object_length,
            environment,
            config,
        };
        publish(initialization.path(), &generation, None)?;
        sync_directory(initialization.path())?;
        before_install()?;
        // Windows refuses to rename a directory containing an open child pin.
        // This private candidate has no external readers yet. Reopen and pin
        // the installed selection before releasing the sole writer lock.
        #[cfg(windows)]
        drop(rdf);
        fs::rename(initialization.path(), &directory)
            .context("Cannot install the prepared snapshot directory")?;
        let _initialized_path = initialization.keep();
        sync_directory(&generation.config.root).context(
            "Snapshot initialization was published, but syncing its parent failed; open to inspect",
        )?;
        #[cfg(not(windows))]
        let current = snapshot_from_generation(generation, rdf);
        #[cfg(windows)]
        let current = open_head(&directory)?;
        let mut writer = Self {
            directory,
            current,
            needs_reopen: false,
            _writer_lock: writer_lock,
        };
        writer.prune_best_effort();
        Ok(writer)
    }

    /// Explicitly copy a legacy managed environment into the new format.
    /// Legacy files are retained and never opened through a writable backend.
    pub fn migrate(root: impl AsRef<Path>) -> Result<Self> {
        let root = fs::canonicalize(root.as_ref())?;
        let legacy = root.join(".ontoenv");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(legacy.join("store.lock"))?;
        FileExt::try_lock_exclusive(&lock)
            .context("Close all legacy environment handles before migration")?;
        let _legacy_lock = WriterLock(lock);
        let mut config = Config::from_file(&legacy.join("ontoenv.json"))?;
        if config.external_graph_store.is_some() || config.temporary {
            bail!("Migration requires a managed persistent RDF5D store");
        }
        if legacy.join(crate::catalog::PENDING_FILE).exists() {
            return Err(crate::errors::CatalogRecoveryError {
                message: format!("Legacy environment has an interrupted mutation; run `ontoenv recover` before migration: {}", fs::read_to_string(legacy.join(crate::catalog::PENDING_FILE))?),
            }.into());
        }
        let catalog_path = legacy.join(crate::catalog::CATALOG_FILE);
        let (environment, expected, _) = if catalog_path.exists() {
            crate::catalog::load(&catalog_path)?
        } else {
            (
                serde_json::from_slice(&fs::read(legacy.join("environment.json"))?)?,
                None,
                HashMap::new(),
            )
        };
        let store_path = legacy.join("store.r5tu");
        let before = crate::io::file_backend_state(&store_path)?;
        if expected
            .as_ref()
            .is_some_and(|expected| expected != &before)
        {
            bail!(
                "Legacy catalog and graph store do not match; recover/reconcile before migration"
            );
        }
        let metadata_paths = [
            legacy.join("ontoenv.json"),
            if catalog_path.exists() {
                catalog_path
            } else {
                legacy.join("environment.json")
            },
        ];
        let hashes: Vec<_> = metadata_paths
            .iter()
            .map(|path| fs::read(path).map(|bytes| blake3::hash(&bytes)))
            .collect::<std::io::Result<_>>()?;
        let source = if store_path.exists() {
            Some(Snapshot::open(&store_path)?)
        } else {
            None
        };
        let physical: HashSet<_> = source
            .as_ref()
            .into_iter()
            .flat_map(|source| source.graph_names())
            .collect();
        // JSON-only catalogs have no persisted revision to distinguish an
        // intentionally empty graph from missing graph data. Require exact
        // graph names rather than silently migrating an incomplete store.
        if !metadata_paths[1].ends_with(crate::catalog::CATALOG_FILE)
            && environment
                .ontologies()
                .keys()
                .any(|id| !physical.contains(id.name().as_str()))
        {
            bail!("legacy metadata does not match backend graph IDs");
        }
        if physical.iter().any(|name| {
            !environment
                .ontologies()
                .keys()
                .any(|id| id.name().as_str() == *name)
        }) {
            bail!("Legacy graph data contains uncatalogued graphs; reconcile before migration");
        }
        if source.is_none() && !environment.ontologies().is_empty() {
            bail!("Legacy graph store is missing");
        }
        config.root = root;
        Self::initialize(config, environment, source.as_ref(), || {
            if crate::io::file_backend_state(&store_path)? != before {
                bail!("Legacy graph store changed during migration");
            }
            for (path, hash) in metadata_paths.iter().zip(&hashes) {
                if blake3::hash(&fs::read(path)?) != *hash {
                    bail!("Legacy metadata changed during migration");
                }
            }
            Ok(())
        })
    }

    /// Open the writer while existing readers retain their immutable snapshots.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = fs::canonicalize(root.as_ref())?;
        let writer_lock = acquire_writer_lock(&root)?;
        let directory = root.join(DIRECTORY);
        let current = open_head(&directory)?;
        let mut writer = Self {
            directory,
            current,
            needs_reopen: false,
            _writer_lock: writer_lock,
        };
        writer.prune_best_effort();
        Ok(writer)
    }

    /// The writer's last confirmed snapshot; staged changes are private.
    /// After a publication error it may lag durable head; reopen to reconcile.
    pub fn snapshot(&self) -> Arc<EnvironmentSnapshot> {
        Arc::clone(&self.current)
    }

    pub(crate) fn selected(&self) -> &EnvironmentSnapshot {
        &self.current
    }

    /// Start a copy-on-write update: only small catalog/config values are cloned.
    /// Untouched graph contents remain in the base snapshot's mapped RDF5D file.
    /// After a publication error, commits are rejected until the writer is reopened.
    pub fn begin_update(&mut self) -> SnapshotUpdate<'_> {
        let candidate = SnapshotCandidate::from_snapshot(&self.current);
        SnapshotUpdate {
            writer: self,
            candidate,
            validate_imports: true,
        }
    }

    /// Publish only configuration, preserving graph/catalog contents. This
    /// validates settings without reinterpreting previously bounded imports.
    /// Derive edits from `self.snapshot().config().clone()` so the effective
    /// canonical storage root is preserved, including on macOS and Windows.
    pub fn publish_config(&mut self, config: Config) -> Result<Arc<EnvironmentSnapshot>> {
        let mut candidate = SnapshotCandidate::from_snapshot(&self.current);
        candidate.config = config;
        self.publish_candidate(candidate)
    }

    pub(crate) fn publish_candidate(
        &mut self,
        candidate: SnapshotCandidate,
    ) -> Result<Arc<EnvironmentSnapshot>> {
        // The source pipeline already applied strictness and explicit depth limits.
        SnapshotUpdate {
            writer: self,
            candidate,
            validate_imports: false,
        }
        .commit()
    }
}

/// The sole mutable candidate, shared by source operations and direct updates.
/// Checkpoints clone catalog/configuration; staged graph buffers stay shared.
#[derive(Clone)]
pub(crate) struct SnapshotCandidate {
    pub environment: Environment,
    pub config: Config,
    pub changes: BTreeMap<String, Option<Arc<StagedGraph>>>,
}

impl SnapshotCandidate {
    pub(crate) fn from_snapshot(snapshot: &EnvironmentSnapshot) -> Self {
        Self {
            environment: snapshot.environment.clone(),
            config: snapshot.config.clone(),
            changes: BTreeMap::new(),
        }
    }
}

/// Private metadata plus graph replacements/deletions over an immutable base.
///
/// Consuming `commit` is the only publication path. Dropping/aborting an update
/// discards its overlay, including previously successful graph ingestions.
pub struct SnapshotUpdate<'a> {
    writer: &'a mut SnapshotWriter,
    candidate: SnapshotCandidate,
    validate_imports: bool,
}

impl SnapshotUpdate<'_> {
    /// Inspect candidate metadata without exposing mutable persistence state.
    pub fn environment(&self) -> &Environment {
        &self.candidate.environment
    }

    /// Edit persistent settings in the same transaction as graphs and aliases.
    /// Storage root, temporary mode and external-backend changes are rejected.
    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.candidate.config
    }

    // Only metadata derived from the candidate graph can enter the catalog.
    fn put_ontology(&mut self, ontology: Ontology, graph: StagedGraph) -> Result<GraphIdentifier> {
        if ontology.location().is_none() {
            bail!("Cannot stage an ontology without a source location");
        }
        let id = ontology.id().clone();
        let name = id.name().as_str().to_owned();
        let old_ids: Vec<_> = self
            .candidate
            .environment
            .ontologies()
            .keys()
            .filter(|old| old.name() == id.name() || old.location() == id.location())
            .cloned()
            .collect();
        // Preserve explicit aliases when replacing metadata/location for an IRI.
        let aliases: Vec<_> = self
            .candidate
            .environment
            .aliases()
            .iter()
            .filter(|(_, target)| old_ids.contains(target))
            .map(|(alias, _)| alias.clone())
            .collect();
        for old in old_ids {
            if old.name() != id.name() {
                // A source that changes its declared IRI must not leave its old
                // graph or source-location index behind in the new generation.
                self.candidate
                    .changes
                    .insert(old.name().as_str().to_owned(), None);
            }
            self.candidate.environment.remove_ontology(&old)?;
        }
        self.candidate.environment.add_ontology(ontology)?;
        for alias in aliases {
            self.candidate.environment.add_alias(&alias, &name)?;
        }
        self.candidate.changes.insert(name, Some(Arc::new(graph)));
        Ok(id)
    }

    /// Replace an existing graph, deriving metadata from its new contents.
    /// Empty graphs retain their identifier but have no imports or RDF metadata.
    /// A different declared ontology IRI requires removing and adding the graph.
    pub fn replace_graph(&mut self, id: &GraphIdentifier, graph: Graph) -> Result<()> {
        if !self.candidate.environment.ontologies().contains_key(id) {
            bail!("Graph {id} is not present in the candidate");
        }
        let mut ontology = Ontology::from_graph(
            &graph,
            id.location().clone(),
            !graph.is_empty() && self.candidate.config.require_ontology_names,
        )?;
        if graph.is_empty() {
            ontology.set_iri(id.name().into_owned());
        } else if ontology.id().name() != id.name() {
            bail!(
                "Replacement declares {}, expected {}; remove and add to change the graph IRI",
                ontology.id().name(),
                id.name()
            );
        }
        self.put_ontology(ontology, StagedGraph::Indexed(Box::new(graph)))?;
        Ok(())
    }

    /// Fetch and stage one source. Imports are validated at commit, not fetched
    /// recursively; stage their sources in the same update when strict is enabled.
    pub fn add_source(&mut self, location: OntologyLocation) -> Result<GraphIdentifier> {
        let location = normalize_source_location(location, &self.candidate.config.root);
        let (bytes, format) = fetch_source(&location, self.candidate.config.offline)?;
        self.add_from_bytes(location, bytes, format)
    }

    /// Parse and stage one source using the exact bytes whose hash is recorded.
    pub fn add_from_bytes(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
    ) -> Result<GraphIdentifier> {
        let location = normalize_source_location(location, &self.candidate.config.root);
        let parsed = parse_ontology_source(
            location,
            bytes,
            format,
            self.candidate.config.require_ontology_names,
        )?;
        let crate::io::ParsedOntology {
            ontology,
            triples,
            bytes,
            ..
        } = parsed;
        drop(bytes);
        self.put_ontology(ontology, StagedGraph::from_triples(triples))
    }

    /// Read staged contents, falling through to the mapped base for untouched graphs.
    pub fn copy_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        if !self.candidate.environment.ontologies().contains_key(id) {
            bail!("Graph {id} is not present in the candidate");
        }
        match self.candidate.changes.get(id.name().as_str()) {
            Some(Some(graph)) => Ok(graph.to_graph()),
            Some(None) => bail!("Graph {id} is deleted in the candidate"),
            None => self.writer.current.copy_graph(id),
        }
    }

    /// Delete both graph contents and metadata. Returns whether the graph existed.
    pub fn remove(&mut self, id: &GraphIdentifier) -> Result<bool> {
        if self.candidate.environment.remove_ontology(id)?.is_none() {
            return Ok(false);
        }
        self.candidate
            .changes
            .insert(id.name().as_str().to_owned(), None);
        Ok(true)
    }

    /// Stage an alias without independently publishing catalog changes.
    pub fn add_alias(&mut self, alias: &str, canonical_iri: &str) -> Result<()> {
        self.candidate.environment.add_alias(alias, canonical_iri)
    }

    /// Remove an alias in the private candidate.
    pub fn remove_alias(&mut self, alias: &str) -> Result<()> {
        self.candidate.environment.remove_alias(alias)?;
        Ok(())
    }

    /// Discard all staged changes without publishing an object or manifest.
    pub fn abort(self) {}

    /// Validate, serialize and atomically publish the complete candidate.
    /// Validation or serialization failure leaves the committed head unchanged.
    /// A publication I/O error may have an uncertain durable outcome: reopen the
    /// writer and inspect its generation before retrying an update.
    pub fn commit(self) -> Result<Arc<EnvironmentSnapshot>> {
        self.commit_before_publish(|| Ok(()))
    }

    // Fault injection stays private: production commits use a no-op hook.
    fn commit_before_publish(
        self,
        before_publish: impl FnOnce() -> Result<()>,
    ) -> Result<Arc<EnvironmentSnapshot>> {
        self.commit_with_publisher(before_publish, publish)
    }

    // The publisher hook lets tests model an error both before and after a
    // successful head replacement, without modifying live mapped files.
    fn commit_with_publisher(
        mut self,
        before_publish: impl FnOnce() -> Result<()>,
        publisher: impl FnOnce(&Path, &Generation, Option<&SnapshotInfo>) -> Result<()>,
    ) -> Result<Arc<EnvironmentSnapshot>> {
        if self.writer.needs_reopen {
            bail!("A previous publication failed; close and reopen the writer before committing");
        }
        self.validate()?;
        let base = &self.writer.current;
        let next = base
            .info
            .generation
            .checked_add(1)
            .context("Generation counter exhausted")?;
        let (object_id, object_length, rdf) = if self.candidate.changes.is_empty() {
            (
                base.object_id.clone(),
                base.object_length,
                Arc::clone(&base.rdf),
            )
        } else {
            write_object(
                &self.writer.directory,
                Some(&base.rdf),
                &self.candidate.changes,
            )?
        };
        self.candidate
            .environment
            .set_default_policy(&self.candidate.config.resolution_policy)?;
        let generation = Generation {
            schema_version: SCHEMA_VERSION,
            info: SnapshotInfo {
                environment_id: base.info.environment_id.clone(),
                generation: next,
                parent_generation: Some(base.info.generation),
                committed_at: Utc::now(),
                opened_at: Utc::now(),
            },
            object_id,
            object_length,
            environment: self.candidate.environment,
            config: self.candidate.config,
        };
        // Prepare the complete in-process snapshot before the publication point.
        let snapshot = snapshot_from_generation(generation, rdf);
        let record = snapshot.record();
        before_publish()?;
        self.writer.needs_reopen = true;
        publisher(&self.writer.directory, &record, Some(&base.info))?;
        self.writer.current = Arc::clone(&snapshot);
        self.writer.needs_reopen = false;
        self.writer.prune_best_effort();
        Ok(snapshot)
    }

    fn validate(&self) -> Result<()> {
        if self.candidate.config.root != self.writer.current.config.root
            || self.candidate.config.temporary
            || self.candidate.config.external_graph_store.is_some()
        {
            bail!("An update cannot change storage root, temporary mode or graph backend");
        }
        self.candidate.config.build_globsets()?;
        self.candidate.config.build_ontology_regexes()?;
        if self.candidate.config.strict && self.validate_imports {
            for ontology in self.candidate.environment.ontologies().values() {
                for import in &ontology.imports {
                    if self
                        .candidate
                        .environment
                        .get_ontology_by_name(import.as_ref())
                        .is_none()
                    {
                        bail!("Unresolved import {import} in {}", ontology.id());
                    }
                }
            }
        }
        // Validate the policy before writing an immutable object.
        if crate::policy::policy_from_name(&self.candidate.config.resolution_policy).is_none() {
            bail!(
                "Unknown resolution policy: {}",
                self.candidate.config.resolution_policy
            );
        }
        Ok(())
    }
}

impl EnvironmentSnapshot {
    fn record(&self) -> Generation {
        Generation {
            schema_version: SCHEMA_VERSION,
            info: self.info.clone(),
            object_id: self.object_id.clone(),
            object_length: self.object_length,
            environment: self.environment.clone(),
            config: self.config.clone(),
        }
    }
}

fn normalize_source_location(location: OntologyLocation, root: &Path) -> OntologyLocation {
    match location {
        OntologyLocation::File(path) => {
            let path = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            OntologyLocation::File(crate::ontology::canonicalize_file_path(&path))
        }
        location => location,
    }
}

// Explicit unlock matters when a concurrent process spawn temporarily inherits
// an open file description before exec: closing only our descriptor can leave
// that lock held by the child even after the writer is dropped.
struct WriterLock(File);

impl Drop for WriterLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn acquire_writer_lock(root: &Path) -> Result<WriterLock> {
    let path = root.join(".ontoenv-snapshots.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    FileExt::try_lock_exclusive(&file).with_context(|| {
        format!(
            "Cannot acquire exclusive lock; another snapshot writer owns {}",
            path.display()
        )
    })?;
    Ok(WriterLock(file))
}

fn read_head(directory: &Path) -> Result<Generation> {
    let _gate = retention::read_gate(directory)?;
    read_head_unlocked(directory)
}

fn read_pointer(directory: &Path) -> Result<Head> {
    let bytes = fs::read(directory.join(CURRENT)).with_context(|| {
        if directory.join("manifest.redb").exists() {
            "This environment uses the earlier redb prototype; automatic migration is unsupported"
        } else {
            "Cannot read the selected OntoEnv version (CURRENT)"
        }
    })?;
    let head: Head = serde_json::from_slice(&bytes).context("Invalid CURRENT file")?;
    if head.schema_version != SCHEMA_VERSION {
        if head.schema_version == 2 {
            bail!("Snapshot reader protocol 2 requires an offline upgrade; close all older processes and run `ontoenv upgrade`");
        }
        bail!("Unsupported CURRENT schema version {}", head.schema_version);
    }
    Ok(head)
}

fn read_head_unlocked(directory: &Path) -> Result<Generation> {
    read_manifest(directory, &read_pointer(directory)?.selected())
}

fn read_manifest(directory: &Path, reference: &ManifestRef) -> Result<Generation> {
    let valid_name = reference
        .manifest
        .strip_prefix("manifest-")
        .and_then(|name| name.strip_suffix(".json"))
        .is_some_and(valid_id);
    if !valid_name {
        bail!("Invalid manifest name in CURRENT");
    }
    let bytes = fs::read(directory.join("manifests").join(&reference.manifest))
        .context("Cannot read the manifest selected by CURRENT")?;
    if blake3::hash(&bytes).to_hex().as_str() != reference.manifest_hash {
        bail!("Selected manifest checksum mismatch");
    }
    let mut generation: Generation = serde_json::from_slice(&bytes)?;
    validate_record(&generation)?;
    // The opened location is operational state, not an authority inherited
    // from the machine/path that originally published this manifest.
    generation.config.root =
        fs::canonicalize(directory.parent().context("Missing environment root")?)?;
    Ok(generation)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

fn validate_record(generation: &Generation) -> Result<()> {
    if generation.schema_version != SCHEMA_VERSION {
        bail!(
            "Unsupported snapshot schema version {}",
            generation.schema_version
        );
    }
    if !valid_id(&generation.object_id) {
        bail!("Invalid snapshot object ID");
    }
    let mut names = HashSet::new();
    for id in generation.environment.ontologies().keys() {
        if !names.insert(id.name().as_str()) {
            bail!(
                "Snapshot catalog contains duplicate graph IRI {}",
                id.name()
            );
        }
    }
    Ok(())
}

fn open_head(directory: &Path) -> Result<Arc<EnvironmentSnapshot>> {
    // Selection and pin acquisition are one short operation relative to GC.
    let _gate = retention::read_gate(directory)?;
    open_generation(read_head_unlocked(directory)?, directory)
}

fn open_generation(generation: Generation, directory: &Path) -> Result<Arc<EnvironmentSnapshot>> {
    let path = directory
        .join("objects")
        .join(&generation.object_id)
        .join("store.r5tu");
    if fs::metadata(&path)?.len() != generation.object_length {
        bail!("Snapshot object length does not match its committed manifest");
    }
    let pin = retention::pin_object(path.parent().context("Missing object directory")?)?;
    let rdf = Arc::new(Snapshot::open_with_guard(&path, pin)?);
    validate_catalog(&generation, &rdf)?;
    Ok(snapshot_from_generation(generation, rdf))
}

fn validate_catalog(generation: &Generation, rdf: &Snapshot) -> Result<()> {
    // Physical graphs may omit empty catalog graphs, but may not be orphaned.
    for name in rdf.graph_names() {
        if !generation
            .environment
            .ontologies()
            .keys()
            .any(|id| id.name().as_str() == name)
        {
            bail!("RDF5D contains graph {name} absent from the committed catalog");
        }
    }
    Ok(())
}

fn snapshot_from_generation(
    mut generation: Generation,
    rdf: Arc<Snapshot>,
) -> Arc<EnvironmentSnapshot> {
    // Rebuild only the nonserialized location index. Explicit aliases are preserved.
    generation.environment.locations.clear();
    let locations: Vec<_> = generation
        .environment
        .ontologies()
        .values()
        .filter_map(|ontology| {
            ontology
                .location()
                .map(|location| (location.clone(), ontology.id().clone()))
        })
        .collect();
    generation.environment.locations.extend(locations);
    generation.info.opened_at = Utc::now();
    Arc::new(EnvironmentSnapshot {
        info: generation.info,
        environment: generation.environment,
        config: generation.config,
        rdf,
        object_id: generation.object_id,
        object_length: generation.object_length,
        catalog_index: OnceLock::new(),
    })
}

fn publish(
    directory: &Path,
    generation: &Generation,
    expected: Option<&SnapshotInfo>,
) -> Result<()> {
    publish_with_hook(directory, generation, expected, |_| Ok(()))
}

// Private injection points cover process exits and I/O failures on either side
// of the one visibility change. Production uses a no-op hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationPhase {
    ManifestWritten,
    ManifestSynced,
    ManifestDurable,
    CurrentWritten,
    CurrentSynced,
    CurrentReplaced,
    Durable,
}

fn publish_with_hook(
    directory: &Path,
    generation: &Generation,
    expected: Option<&SnapshotInfo>,
    at: impl FnMut(PublicationPhase) -> Result<()>,
) -> Result<()> {
    validate_record(generation)?;
    // The stable exclusive writer lock serializes this check and replacement.
    // This is not a compare-and-swap for writers bypassing that lock.
    let previous = if let Some(expected) = expected {
        let head = read_pointer(directory)?;
        let current = read_manifest(directory, &head.selected())?;
        if current.info.environment_id != expected.environment_id
            || current.info.generation != expected.generation
        {
            bail!("Committed head changed since update began; reopen the writer");
        }
        Some(head.selected())
    } else {
        if directory.join(CURRENT).try_exists()? {
            bail!("Unexpected snapshot manifest head; reopen the writer");
        }
        None
    };
    publish_manifest_with_hook(directory, generation, previous, at)
}

fn publish_manifest_with_hook(
    directory: &Path,
    generation: &Generation,
    previous: Option<ManifestRef>,
    mut at: impl FnMut(PublicationPhase) -> Result<()>,
) -> Result<()> {
    let bytes = serde_json::to_vec(generation)?;
    let manifests = directory.join("manifests");
    let mut manifest = tempfile::Builder::new()
        .prefix("manifest-")
        .suffix(".json")
        .tempfile_in(&manifests)?;
    manifest.write_all(&bytes)?;
    at(PublicationPhase::ManifestWritten)?;
    manifest.as_file().sync_all()?;
    at(PublicationPhase::ManifestSynced)?;
    let (file, manifest_path) = manifest.keep()?;
    drop(file);
    sync_directory(&manifests)?;
    at(PublicationPhase::ManifestDurable)?;

    let head = Head {
        schema_version: SCHEMA_VERSION,
        manifest: manifest_path
            .file_name()
            .context("Manifest has no file name")?
            .to_str()
            .context("Manifest name is not UTF-8")?
            .to_owned(),
        manifest_hash: blake3::hash(&bytes).to_hex().to_string(),
        previous,
    };
    // This tempfile is on the same filesystem as CURRENT. Close the writing
    // handle and clear Windows temporary attributes before replacement.
    let mut pointer = tempfile::Builder::new()
        .prefix(".CURRENT-")
        .tempfile_in(directory)?;
    pointer.write_all(&serde_json::to_vec(&head)?)?;
    at(PublicationPhase::CurrentWritten)?;
    pointer.as_file().sync_all()?;
    at(PublicationPhase::CurrentSynced)?;
    let (file, pointer_path) = pointer.keep()?;
    drop(file);
    // Unlike tempfile's MoveFileExW-only persist, std::fs::rename has a Windows
    // FileRenameInfoEx/POSIX fallback when the old CURRENT has open readers.
    // Existing handles keep reading the old inode; new opens select the new one.
    let replacement = fs::rename(&pointer_path, directory.join(CURRENT));
    if replacement.is_err() {
        let _ = fs::remove_file(&pointer_path);
    }
    replacement.context("CURRENT replacement failed; reopen before retrying")?;
    at(PublicationPhase::CurrentReplaced)?;
    sync_directory(directory)
        .context("CURRENT was replaced but syncing its parent failed; reopen to inspect")?;
    at(PublicationPhase::Durable)?;
    Ok(())
}

fn write_object(
    directory: &Path,
    base: Option<&Snapshot>,
    changes: &BTreeMap<String, Option<Arc<StagedGraph>>>,
) -> Result<(String, u64, Arc<Snapshot>)> {
    let objects = directory.join("objects");
    // A randomly named private object directory cannot collide with an abandoned
    // commit attempt. It is unreferenced until CURRENT selects its manifest.
    let staging = tempfile::Builder::new()
        .prefix("object-")
        .tempdir_in(&objects)?;
    let path = staging.path().join("store.r5tu");
    let mut writer = StreamingWriter::new(
        &path,
        WriterOptions {
            zstd: true,
            with_crc: true,
        },
    );
    if let Some(base) = base {
        for group in base.file().enumerate_all()? {
            if changes.contains_key(&group.graphname) {
                continue;
            }
            for triple in base.file().oxigraph_triples(group.gid)? {
                let triple = triple?;
                writer.add(Quint {
                    id: group.id.clone(),
                    gname: group.graphname.clone(),
                    s: oxigraph_subject_to_r5term(triple.subject),
                    p: rdf5d::Term::Iri(triple.predicate.as_str().to_owned()),
                    o: oxigraph_object_to_r5term(triple.object),
                })?;
            }
        }
    }
    for (name, graph) in changes {
        if let Some(graph) = graph {
            for triple in graph.iter() {
                let triple = triple.into_owned();
                writer.add(Quint {
                    id: name.clone(),
                    gname: name.clone(),
                    s: oxigraph_subject_to_r5term(triple.subject),
                    p: rdf5d::Term::Iri(triple.predicate.as_str().to_owned()),
                    o: oxigraph_object_to_r5term(triple.object),
                })?;
            }
        }
    }
    writer.finalize()?;
    retention::create_pin(staging.path())?;
    let rdf = Arc::new(Snapshot::open_with_guard(
        &path,
        retention::pin_object(staging.path())?,
    )?);
    let length = fs::metadata(&path)?.len();
    sync_directory(staging.path())?;
    let object_id = staging
        .path()
        .file_name()
        .context("Object directory has no name")?
        .to_str()
        .context("Object directory name is not UTF-8")?
        .to_owned();
    // From here onward, failures may leave an unreferenced, complete object.
    let _published_path = staging.keep();
    sync_directory(&objects)?;
    Ok((object_id, length, rdf))
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::model::NamedNode;
    use std::process::Command;

    fn config(root: &Path) -> Config {
        Config::builder()
            .root(root.to_path_buf())
            .temporary(false)
            .strict(true)
            .offline(true)
            .locations(vec![])
            .build()
            .unwrap()
    }

    fn stage<'a>(writer: &'a mut SnapshotWriter, value: &str) -> Result<SnapshotUpdate<'a>> {
        let mut update = writer.begin_update();
        update.add_from_bytes(
            OntologyLocation::InMemory {
                identifier: "urn:a".to_owned(),
            },
            format!(
                "<urn:a> a <http://www.w3.org/2002/07/owl#Ontology> ; <urn:value> \"{value}\" ."
            )
            .into_bytes(),
            Some(RdfFormat::Turtle),
        )?;
        Ok(update)
    }

    #[test]
    #[ignore = "independent reader lifetime helper"]
    fn pinned_reader_worker() -> Result<()> {
        use std::io::BufRead;
        let root = PathBuf::from(std::env::var("ONTOENV_PIN_ROOT")?);
        let reader = SnapshotReader::open(&root)?;
        let rdf = reader.snapshot().rdf();
        let scan = rdf.grouped_scan(rdf5d::Pattern::ANY, rdf5d::Scope::All)?;
        drop(rdf);
        drop(reader);
        fs::write(root.join("reader-ready"), b"ready")?;
        let mut command = String::new();
        std::io::stdin().lock().read_line(&mut command)?;
        assert_eq!(scan.count(), 2);
        Ok(())
    }

    #[test]
    fn independent_iterators_pin_files_across_writer_restarts_and_process_death() -> Result<()> {
        use std::process::Stdio;
        for abrupt in [false, true] {
            let dir = tempfile::tempdir()?;
            let mut writer = SnapshotWriter::create(config(dir.path()))?;
            stage(&mut writer, "old")?.commit()?;
            let old_path = writer
                .directory
                .join("objects")
                .join(&writer.current.object_id);
            let mut child = Command::new(std::env::current_exe()?)
                .args([
                    "--ignored",
                    "--exact",
                    "snapshots::tests::pinned_reader_worker",
                ])
                .env("ONTOENV_PIN_ROOT", dir.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !dir.path().join("reader-ready").exists() {
                assert!(
                    child.try_wait()?.is_none(),
                    "reader exited before acquiring its view"
                );
                assert!(
                    std::time::Instant::now() < deadline,
                    "reader acquisition timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            stage(&mut writer, "new")?.commit()?;
            stage(&mut writer, "newest")?.commit()?;
            drop(writer);
            let mut writer = SnapshotWriter::open(dir.path())?;
            assert!(old_path.exists());
            assert_eq!(writer.prune()?.pinned_objects, 1);
            if abrupt {
                child.kill()?;
                child.wait()?;
            } else {
                child.stdin.take().unwrap().write_all(b"release\n")?;
                assert!(child.wait()?.success());
            }
            assert!(writer.prune()?.errors.is_empty());
            assert!(!old_path.exists());
        }
        Ok(())
    }

    #[test]
    fn owned_iterator_retains_backing_after_snapshot_and_reader_drop() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut writer = SnapshotWriter::create(config(dir.path()))?;
        stage(&mut writer, "old")?.commit()?;
        let old_path = writer
            .directory
            .join("objects")
            .join(&writer.current.object_id);
        let snapshot = SnapshotReader::open(dir.path())?.snapshot();
        let rdf = snapshot.rdf();
        let mut iterator = rdf.grouped_scan(rdf5d::Pattern::ANY, rdf5d::Scope::All)?;
        drop(rdf);
        drop(snapshot);
        stage(&mut writer, "new")?.commit()?;
        stage(&mut writer, "newest")?.commit()?;
        assert!(old_path.exists());
        assert!(iterator.next().is_some());
        drop(iterator);
        writer.prune()?;
        assert!(!old_path.exists());
        Ok(())
    }

    #[test]
    fn cleanup_retries_retirement_and_skips_snapshot_acquisition() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut writer = SnapshotWriter::create(config(dir.path()))?;
        let gate = retention::read_gate(&writer.directory)?;
        assert!(writer.prune()?.acquisition_busy);
        drop(gate);
        // Simulate an orphan and a failed retirement (destination occupied).
        let object = writer.directory.join("objects/object-abandoned");
        fs::create_dir(&object)?;
        fs::write(object.join("store.r5tu"), b"aborted")?;
        let blocker = writer.directory.join("trash/object-abandoned");
        fs::write(&blocker, b"block retirement")?;
        assert!(!writer.prune()?.errors.is_empty());
        assert!(object.exists());
        // Cleanup failure does not roll back an otherwise successful commit.
        stage(&mut writer, "published despite garbage")?.commit()?;
        assert_eq!(writer.current.info.generation, 1);
        assert!(object.exists());
        fs::remove_file(&blocker)?;
        assert_eq!(writer.prune()?.removed_objects, 1);
        assert!(!object.exists());
        // A retired directory left by an interrupted deletion is retried too.
        fs::create_dir(&blocker)?;
        fs::write(blocker.join("partial"), b"leftover")?;
        drop(writer);
        let _writer = SnapshotWriter::open(dir.path())?;
        assert!(!blocker.exists());
        Ok(())
    }

    #[test]
    fn protocol_two_requires_explicit_offline_upgrade_without_rewriting_graphs() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut writer = SnapshotWriter::create(config(dir.path()))?;
        stage(&mut writer, "old")?.commit()?;
        let directory = writer.directory.clone();
        let mut record = writer.current.record();
        let path = directory
            .join("objects")
            .join(&record.object_id)
            .join("store.r5tu");
        let graph_hash = blake3::hash(&fs::read(&path)?);
        drop(writer);
        // Construct the exact previous reader protocol with no pin/gate files.
        let mut head: Head = serde_json::from_slice(&fs::read(directory.join(CURRENT))?)?;
        record.schema_version = 2;
        let bytes = serde_json::to_vec(&record)?;
        fs::write(directory.join("manifests").join(&head.manifest), &bytes)?;
        head.schema_version = 2;
        head.previous = None;
        head.manifest_hash = blake3::hash(&bytes).to_hex().to_string();
        let pointer = serde_json::to_vec(&head)?;
        fs::write(directory.join(CURRENT), &pointer)?;
        fs::remove_file(directory.join("READERS"))?;
        fs::remove_file(path.parent().unwrap().join(".pin"))?;
        assert!(SnapshotReader::open(dir.path())
            .err()
            .unwrap()
            .to_string()
            .contains("offline upgrade"));
        assert!(SnapshotWriter::open(dir.path()).is_err());
        assert_eq!(fs::read(directory.join(CURRENT))?, pointer);
        let mut writer = SnapshotWriter::upgrade(dir.path())?;
        assert_eq!(writer.current.info.generation, record.info.generation);
        assert_eq!(writer.current.object_id, record.object_id);
        assert_eq!(blake3::hash(&fs::read(path)?), graph_hash);
        let held = writer.snapshot();
        stage(&mut writer, "new")?.commit()?;
        stage(&mut writer, "newest")?.commit()?;
        assert_eq!(writer.prune()?.pinned_objects, 1);
        drop(held);
        assert!(writer.prune()?.errors.is_empty());
        Ok(())
    }

    #[test]
    #[cfg(windows)]
    fn retired_deletion_retries_after_windows_handle_closes() -> Result<()> {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir()?;
        let mut writer = SnapshotWriter::create(config(dir.path()))?;
        let retired = writer.directory.join("trash/object-interrupted");
        fs::create_dir(&retired)?;
        let path = retired.join("store.r5tu");
        fs::write(&path, b"retired")?;
        // A non-protocol handle can prevent deletion without owning a pin.
        let handle = OpenOptions::new().read(true).share_mode(1).open(path)?;
        let report = writer.prune()?;
        assert_eq!(report.pending_objects, 1);
        assert!(!report.errors.is_empty());
        drop(handle);
        drop(writer);
        let _writer = SnapshotWriter::open(dir.path())?;
        assert!(!retired.exists());
        Ok(())
    }

    #[test]
    fn cleanup_preserves_exact_previous_publication_and_shared_metadata_backing() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut writer = SnapshotWriter::create(config(dir.path()))?;
        stage(&mut writer, "old")?.commit()?;
        let before = read_pointer(&writer.directory)?.selected();
        let before_object = writer.current.object_id.clone();
        let result = stage(&mut writer, "aborted")?.commit_with_publisher(
            || Ok(()),
            |directory, record, expected| {
                publish_with_hook(directory, record, expected, |phase| {
                    if phase == PublicationPhase::ManifestDurable {
                        bail!("injected failure");
                    }
                    Ok(())
                })
            },
        );
        assert!(result.is_err());
        drop(writer);
        let mut writer = SnapshotWriter::open(dir.path())?;
        stage(&mut writer, "new")?.commit()?;
        let head = read_pointer(&writer.directory)?;
        assert_eq!(head.previous.unwrap().manifest, before.manifest);
        assert!(writer
            .directory
            .join("objects")
            .join(before_object)
            .exists());
        let snapshot = writer.snapshot();
        let mut config = snapshot.config().clone();
        config.namespaces.insert("new".into(), "urn:new:".into());
        writer.publish_config(config)?;
        assert!(Arc::ptr_eq(&snapshot.rdf(), &writer.snapshot().rdf()));
        assert_eq!(fs::read_dir(writer.directory.join("objects"))?.count(), 1);
        assert_eq!(fs::read_dir(writer.directory.join("manifests"))?.count(), 2);
        Ok(())
    }

    #[test]
    #[ignore = "child process fault injection helper"]
    fn crash_worker() -> Result<()> {
        let root = PathBuf::from(std::env::var("ONTOENV_SNAPSHOT_CRASH_ROOT")?);
        let phase = std::env::var("ONTOENV_SNAPSHOT_CRASH_PHASE")?;
        let mut writer = SnapshotWriter::open(root)?;
        let mut update = stage(&mut writer, "new")?;
        update.add_alias("urn:committed-alias", "urn:a")?;
        update
            .config_mut()
            .namespaces
            .insert("revision".into(), "urn:new:".into());
        if phase == "ObjectReady" {
            update.commit_before_publish(|| std::process::exit(71))?;
        } else {
            update.commit_with_publisher(
                || Ok(()),
                |directory, generation, expected| {
                    publish_with_hook(directory, generation, expected, |at| {
                        if format!("{at:?}") == phase {
                            std::process::exit(71);
                        }
                        Ok(())
                    })
                },
            )?;
        }
        unreachable!()
    }

    #[test]
    fn crash_at_each_publication_step_allows_read_only_open_without_recovery() -> Result<()> {
        for phase in [
            "ObjectReady",
            "ManifestWritten",
            "ManifestSynced",
            "ManifestDurable",
            "CurrentWritten",
            "CurrentSynced",
            "CurrentReplaced",
            "Durable",
        ] {
            let published = matches!(phase, "CurrentReplaced" | "Durable");
            let dir = tempfile::tempdir()?;
            let mut writer = SnapshotWriter::create(config(dir.path()))?;
            stage(&mut writer, "old")?.commit()?;
            let mut reader = SnapshotReader::open(dir.path())?;
            let retained = reader.snapshot();
            let id = retained.resolve(NamedNode::new("urn:a")?.as_ref()).unwrap();
            drop(writer);
            let output = Command::new(std::env::current_exe()?)
                .args([
                    "--ignored",
                    "--exact",
                    "snapshots::tests::crash_worker",
                    "--nocapture",
                ])
                .env("ONTOENV_SNAPSHOT_CRASH_ROOT", dir.path())
                .env("ONTOENV_SNAPSHOT_CRASH_PHASE", phase)
                .output()?;
            assert_eq!(
                output.status.code(),
                Some(71),
                "child failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            // Open read-only before any writer recovery or cleanup. CURRENT is
            // the sole authority, regardless of abandoned higher manifests.
            let fresh = SnapshotReader::open(dir.path())?;
            assert_eq!(
                fresh.snapshot().info().generation,
                if published { 2 } else { 1 },
                "{phase}"
            );
            assert_eq!(reader.refresh()?, published, "{phase}");
            let snapshot = reader.snapshot();
            assert_eq!(
                snapshot
                    .environment()
                    .resolve_alias("urn:committed-alias")
                    .is_some(),
                published
            );
            assert_eq!(
                snapshot
                    .config()
                    .namespaces
                    .get("revision")
                    .map(String::as_str),
                if published { Some("urn:new:") } else { None }
            );
            let expected = if !published { "\"old\"" } else { "\"new\"" };
            assert!(snapshot
                .copy_graph(&id)?
                .iter()
                .any(|t| t.object.to_string() == expected));
            assert!(retained
                .copy_graph(&id)?
                .iter()
                .any(|t| t.object.to_string() == "\"old\""));
            // Reopening a writer needs no repair and can immediately publish.
            let mut writer = SnapshotWriter::open(dir.path())?;
            stage(&mut writer, "next")?.commit()?;
            assert_eq!(
                writer.snapshot().info().generation,
                if published { 3 } else { 2 }
            );
        }
        Ok(())
    }

    #[test]
    fn publication_failure_retains_old_head_and_complete_unreferenced_object() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut writer = SnapshotWriter::create(config(dir.path()))?;
        stage(&mut writer, "old")?.commit()?;
        let old = writer.snapshot();
        let count_before = fs::read_dir(dir.path().join(DIRECTORY).join("objects"))?.count();
        let result = stage(&mut writer, "unpublished")?
            .commit_before_publish(|| bail!("injected publication failure"));
        assert!(result.is_err());
        assert_eq!(writer.snapshot().info().generation, old.info().generation);
        assert_eq!(
            SnapshotReader::open(dir.path())?
                .snapshot()
                .info()
                .generation,
            old.info().generation
        );
        assert_eq!(
            fs::read_dir(dir.path().join(DIRECTORY).join("objects"))?.count(),
            count_before + 1
        );
        // A failure known to precede publication does not disable the writer.
        stage(&mut writer, "retry")?.commit()?;
        Ok(())
    }

    #[test]
    fn publication_error_requires_reopen_for_either_durable_outcome() -> Result<()> {
        for did_commit in [false, true] {
            let dir = tempfile::tempdir()?;
            let mut writer = SnapshotWriter::create(config(dir.path()))?;
            stage(&mut writer, "old")?.commit()?;
            let mut reader = SnapshotReader::open(dir.path())?;
            let retained = reader.snapshot();
            let id = retained.resolve(NamedNode::new("urn:a")?.as_ref()).unwrap();
            let result = stage(&mut writer, "new")?.commit_with_publisher(
                || Ok(()),
                |directory, generation, expected| {
                    publish_with_hook(directory, generation, expected, |phase| {
                        if phase
                            == if did_commit {
                                PublicationPhase::CurrentReplaced
                            } else {
                                PublicationPhase::CurrentSynced
                            }
                        {
                            bail!("injected uncertain publication outcome");
                        }
                        Ok(())
                    })
                },
            );
            assert!(result.is_err());
            assert_eq!(writer.snapshot().info().generation, 1);
            assert_eq!(reader.refresh()?, did_commit);
            let objects_before = fs::read_dir(dir.path().join(DIRECTORY).join("objects"))?.count();
            let error = stage(&mut writer, "retry")?.commit().unwrap_err();
            assert!(error.to_string().contains("close and reopen"));
            assert_eq!(
                fs::read_dir(dir.path().join(DIRECTORY).join("objects"))?.count(),
                objects_before,
                "rejected retry must not serialize another object"
            );
            assert!(retained
                .copy_graph(&id)?
                .iter()
                .any(|t| t.object.to_string() == "\"old\""));
            drop(writer);
            let mut writer = SnapshotWriter::open(dir.path())?;
            assert_eq!(
                writer.snapshot().info().generation,
                if did_commit { 2 } else { 1 }
            );
            let expected = if did_commit { "\"new\"" } else { "\"old\"" };
            assert!(writer
                .snapshot()
                .copy_graph(&id)?
                .iter()
                .any(|t| t.object.to_string() == expected));
            stage(&mut writer, "retry")?.commit()?;
        }
        Ok(())
    }

    #[test]
    fn publication_replaces_current_while_an_old_pointer_is_open() -> Result<()> {
        use std::io::{Read, Seek, SeekFrom};

        let dir = tempfile::tempdir()?;
        let mut writer = SnapshotWriter::create(config(dir.path()))?;
        stage(&mut writer, "old")?.commit()?;
        let current = dir.path().join(DIRECTORY).join(CURRENT);
        let before = fs::read(&current)?;
        let mut open_pointer = File::open(&current)?;
        stage(&mut writer, "new")?.commit()?;
        assert_ne!(fs::read(&current)?, before);
        open_pointer.seek(SeekFrom::Start(0))?;
        let mut retained = Vec::new();
        open_pointer.read_to_end(&mut retained)?;
        assert_eq!(retained, before);
        assert_eq!(
            SnapshotReader::open(dir.path())?
                .snapshot()
                .info()
                .generation,
            2
        );
        Ok(())
    }

    #[test]
    fn invalid_selected_manifest_never_falls_back_or_replaces_a_reader() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut writer = SnapshotWriter::create(config(dir.path()))?;
        stage(&mut writer, "old")?.commit()?;
        let mut reader = SnapshotReader::open(dir.path())?;
        let retained = reader.snapshot();
        stage(&mut writer, "new")?.commit()?;
        let directory = dir.path().join(DIRECTORY);
        let selected = fs::read(directory.join(CURRENT))?;
        let mut cases = vec![b"not json".to_vec()];
        for (name, hash, schema) in [
            ("../outside.json", "unused", SCHEMA_VERSION),
            ("manifest-../outside.json", "unused", SCHEMA_VERSION),
            ("manifest-.json", "unused", SCHEMA_VERSION),
            ("manifest-missing.json", "unused", SCHEMA_VERSION),
        ] {
            cases.push(serde_json::to_vec(&Head {
                schema_version: schema,
                manifest: name.into(),
                manifest_hash: hash.into(),
                previous: None,
            })?);
        }
        let mut wrong_schema: Head = serde_json::from_slice(&selected)?;
        wrong_schema.schema_version += 1;
        cases.push(serde_json::to_vec(&wrong_schema)?);
        let mut wrong_hash: Head = serde_json::from_slice(&selected)?;
        wrong_hash.manifest_hash = "0".repeat(64);
        cases.push(serde_json::to_vec(&wrong_hash)?);
        // A checksum-valid manifest must also pass schema, object-path and JSON
        // validation. None of these bad manifests is mapped graph data.
        let mut wrong_record = read_head(&directory)?;
        wrong_record.schema_version += 1;
        let mut wrong_object = read_head(&directory)?;
        wrong_object.object_id = "../outside".into();
        for bytes in [
            serde_json::to_vec(&wrong_record)?,
            serde_json::to_vec(&wrong_object)?,
            b"not json".to_vec(),
        ] {
            let mut manifest = tempfile::Builder::new()
                .prefix("manifest-")
                .suffix(".json")
                .tempfile_in(directory.join("manifests"))?;
            manifest.write_all(&bytes)?;
            cases.push(serde_json::to_vec(&Head {
                schema_version: SCHEMA_VERSION,
                manifest: manifest
                    .path()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .into(),
                manifest_hash: blake3::hash(&bytes).to_hex().to_string(),
                previous: None,
            })?);
            let (file, _) = manifest.keep()?;
            drop(file);
        }
        for bytes in cases {
            fs::write(directory.join(CURRENT), bytes)?;
            assert!(SnapshotReader::open(dir.path()).is_err());
            assert!(reader.is_current().is_err());
            assert!(reader.refresh().is_err());
            assert!(Arc::ptr_eq(&reader.snapshot(), &retained));
        }
        fs::write(directory.join(CURRENT), selected)?;
        assert!(reader.refresh()?);
        assert_eq!(reader.snapshot().info().generation, 2);
        Ok(())
    }

    #[test]
    fn dropped_writer_explicitly_unlocks_even_with_an_inherited_descriptor() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let lock = acquire_writer_lock(dir.path())?;
        let inherited = lock.0.try_clone()?;
        assert!(acquire_writer_lock(dir.path()).is_err());
        drop(lock);
        let next = acquire_writer_lock(dir.path())?;
        drop(inherited);
        assert!(acquire_writer_lock(dir.path()).is_err());
        drop(next);
        acquire_writer_lock(dir.path())?;
        Ok(())
    }

    #[test]
    fn earlier_redb_prototype_is_rejected_without_modifying_its_files() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let directory = dir.path().join(DIRECTORY);
        fs::create_dir(&directory)?;
        fs::write(directory.join("manifest.redb"), b"earlier prototype")?;
        let error = SnapshotReader::open(dir.path())
            .err()
            .context("expected migration error")?;
        assert!(error
            .to_string()
            .contains("automatic migration is unsupported"));
        assert!(SnapshotWriter::open(dir.path()).is_err());
        assert!(!directory.join(CURRENT).exists());
        assert_eq!(
            fs::read(directory.join("manifest.redb"))?,
            b"earlier prototype"
        );
        Ok(())
    }

    // Used only by lib/tests/support/snapshot_vm.py inside a disposable guest.
    // A phase notification parks the guest process until the host kills QEMU;
    // unlike process-exit tests, the guest kernel cannot finish flushing afterward.
    #[test]
    #[ignore = "disposable VM durability helper"]
    fn durability_vm_worker() -> Result<()> {
        fn cut(phase: &str) -> ! {
            println!("ONTOENV_VM_CUT {phase}");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
        let mode = std::env::var("ONTOENV_VM_MODE")?;
        let root = Path::new("/data/environment");
        if mode == "seed" {
            let mut writer = SnapshotWriter::create(config(root))?;
            let mut update = stage(&mut writer, "old")?;
            update
                .config_mut()
                .namespaces
                .insert("revision".into(), "urn:old:".into());
            update.commit()?;
            println!("ONTOENV_VM_SEEDED");
        } else if mode == "update" {
            let target = std::env::var("ONTOENV_VM_PHASE")?;
            let mut writer = SnapshotWriter::open(root)?;
            let mut update = stage(&mut writer, "new")?;
            update
                .config_mut()
                .namespaces
                .insert("revision".into(), "urn:new:".into());
            update.add_alias("urn:new-alias", "urn:a")?;
            update.commit_with_publisher(
                || {
                    if target == "ObjectReady" {
                        cut("ObjectReady");
                    }
                    Ok(())
                },
                |directory, generation, expected| {
                    publish_with_hook(directory, generation, expected, |phase| {
                        if format!("{phase:?}") == target {
                            cut(&target);
                        }
                        Ok(())
                    })
                },
            )?;
            // Acknowledged means the public commit has returned success.
            if target == "Acknowledged" {
                cut("Acknowledged");
            }
            bail!("Unknown VM crash phase {target}");
        } else if mode == "verify" {
            // The harness mounts /data read-only. Do not open a writer here.
            let reader = SnapshotReader::open(root)?;
            let snapshot = reader.snapshot();
            let generation = snapshot.info().generation;
            let expected = std::env::var("ONTOENV_VM_EXPECTED")?;
            match expected.as_str() {
                "old" => assert_eq!(generation, 1),
                "new" => assert_eq!(generation, 2),
                "either" => assert!(matches!(generation, 1 | 2)),
                _ => bail!("Unknown expected VM outcome"),
            }
            let id = snapshot
                .resolve(NamedNode::new("urn:a")?.as_ref())
                .context("missing ontology")?;
            let value = if generation == 1 { "old" } else { "new" };
            assert!(snapshot
                .copy_graph(&id)?
                .iter()
                .any(|t| t.object.to_string() == format!("\"{value}\"")));
            assert_eq!(
                snapshot.config().namespaces["revision"],
                format!("urn:{value}:")
            );
            assert_eq!(
                snapshot
                    .resolve(NamedNode::new("urn:new-alias")?.as_ref())
                    .is_some(),
                generation == 2
            );
            println!("ONTOENV_VM_VERIFIED {generation}");
        } else {
            bail!("Unknown VM mode {mode}");
        }
        Ok(())
    }
}
