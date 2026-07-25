//! Defines the main OntoEnv API struct and its methods for managing the ontology environment.
//! This includes loading, saving, updating, and querying the environment.

use crate::catalog;
use crate::config::Config;
use crate::consts::{IMPORTS, ONTOLOGY, TYPE};
use crate::doctor::{
    ConflictingPrefixes, Doctor, DuplicateOntology, OntologyDeclaration, OntologyProblem,
};
use crate::environment::Environment;
use crate::errors::{CatalogRecoveryError, ExternalStoreChangedError, StoreCapabilityError};
use crate::options::{Overwrite, RefreshStrategy};
use crate::transform;
use crate::ToUriString;
use crate::{EnvironmentStatus, FailedImport};
use chrono::prelude::*;
use fs2::FileExt;
use oxigraph::io::RdfFormat;
use oxigraph::model::{
    Dataset, Graph, GraphNameRef, NamedNode, NamedNodeRef, NamedOrBlankNodeRef, Quad, QuadRef,
    TermRef, TripleRef,
};
use oxigraph::store::Store;
use petgraph::visit::EdgeRef;
use regex::Regex;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::path::PathBuf;

use crate::io::GraphIO;
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use crate::progress::ProgressReporter;
use anyhow::{anyhow, Result};
use blake3;
use log::{debug, error, info, warn};
use petgraph::graph::{Graph as DiGraph, NodeIndex};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;

#[derive(Clone, Debug)]
enum PendingImport {
    FromLocation {
        location: OntologyLocation,
        overwrite: Overwrite,
        required: bool,
        depth: usize,
    },
    FromBytes {
        location: OntologyLocation,
        overwrite: Overwrite,
        required: bool,
        depth: usize,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
    },
}

impl PendingImport {
    fn from_location(
        location: OntologyLocation,
        overwrite: Overwrite,
        required: bool,
        depth: usize,
    ) -> Self {
        Self::FromLocation {
            location,
            overwrite,
            required,
            depth,
        }
    }

    fn from_bytes(
        location: OntologyLocation,
        overwrite: Overwrite,
        required: bool,
        depth: usize,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
    ) -> Self {
        Self::FromBytes {
            location,
            overwrite,
            required,
            depth,
            bytes,
            format,
        }
    }

    fn meta(&self) -> (&OntologyLocation, bool, usize) {
        match self {
            Self::FromLocation {
                location,
                required,
                depth,
                ..
            }
            | Self::FromBytes {
                location,
                required,
                depth,
                ..
            } => (location, *required, *depth),
        }
    }
}

/// Initializes logging for the ontoenv library.
///
/// This function checks for the `ONTOENV_LOG` environment variable. If it is set,
/// `RUST_LOG` is set to its value. `ONTOENV_LOG` takes precedence over `RUST_LOG`.
/// The logger initialization (e.g., `env_logger::init()`) must be called after
/// this function for the log level to take effect.
pub fn init_logging() {
    // Allow ONTOENV_LOG to override RUST_LOG for consistent CLI defaults.
    if let Ok(log_level) = std::env::var("ONTOENV_LOG") {
        std::env::set_var("RUST_LOG", log_level);
    }
}

/// Searches for the .ontoenv directory in the given directory and then recursively up its parent directories.
/// Returns the path to the directory containing the .ontoenv directory if found.
pub fn find_ontoenv_root_from(start_dir: &Path) -> Option<PathBuf> {
    // Walk up the directory tree to find the nearest .ontoenv marker.
    let mut current_dir = Some(start_dir);
    while let Some(dir) = current_dir {
        if dir.join(".ontoenv").is_dir() {
            return Some(dir.to_path_buf());
        }
        current_dir = dir.parent();
    }
    None
}

/// Searches for the .ontoenv directory in the current directory and then recursively up its parent directories.
/// Returns the path to the directory containing the .ontoenv directory if found.
pub fn find_ontoenv_root() -> Option<PathBuf> {
    // Resolve from current working directory for CLI friendliness.
    let start_dir = std::env::current_dir().ok()?;
    find_ontoenv_root_from(&start_dir)
}

/// These are the different ways to refer to an ontology: either
/// by a location (file or URL), or the name of the graph (IRI)
pub enum ResolveTarget {
    Location(OntologyLocation),
    Graph(NamedNode),
}

/// Represents the result of a union graph operation.
/// Contains the resulting dataset, the identifiers of the graphs included,
/// and any imports that failed during the process.
pub struct UnionGraph {
    pub dataset: Dataset,
    pub graph_ids: Vec<GraphIdentifier>,
    pub failed_imports: Option<Vec<FailedImport>>,
    pub namespace_map: HashMap<String, String>,
}

impl UnionGraph {
    /// Returns the total number of triples in the union graph dataset.
    pub fn len(&self) -> usize {
        // Delegate to Dataset length to keep semantics consistent.
        self.dataset.len()
    }

    /// Returns true if the union dataset is empty.
    pub fn is_empty(&self) -> bool {
        self.dataset.is_empty()
    }

    /// Returns the union of all namespace maps from the ontologies in the graph.
    pub fn get_namespace_map(&self) -> &HashMap<String, String> {
        // Expose the merged prefix map for tooling and serialization.
        &self.namespace_map
    }
}

pub struct Stats {
    pub num_triples: usize,
    pub num_graphs: usize,
    pub num_ontologies: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncMode {
    Incremental,
    Targeted,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub mode: SyncMode,
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
    pub still_pending: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ImportPaths {
    Present(Vec<Vec<GraphIdentifier>>),
    Missing {
        importers: Vec<Vec<GraphIdentifier>>,
    },
}

#[derive(Default)]
struct BatchState {
    depth: usize,
    seen_locations: HashSet<OntologyLocation>,
}

impl BatchState {
    fn begin(&mut self) {
        if self.depth == 0 {
            self.seen_locations.clear();
        }
        self.depth += 1;
    }

    fn end(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn has_seen(&self, location: &OntologyLocation) -> bool {
        self.seen_locations.contains(location)
    }

    fn mark_seen(&mut self, location: &OntologyLocation) {
        self.seen_locations.insert(location.clone());
    }
}

struct BatchScope<'a> {
    env: &'a mut OntoEnv,
    completed: bool,
    outermost: bool,
}

#[derive(Default)]
struct OntologyFilters {
    include: Vec<Regex>,
    exclude: Vec<Regex>,
}

impl OntologyFilters {
    fn allow(&self, id: &GraphIdentifier) -> bool {
        let iri = id.to_uri_string();
        if self.exclude.iter().any(|re| re.is_match(&iri)) {
            false
        } else if self.include.is_empty() {
            true
        } else {
            self.include.iter().any(|re| re.is_match(&iri))
        }
    }
}

impl<'a> BatchScope<'a> {
    fn enter(env: &'a mut OntoEnv) -> Result<Self> {
        let outermost = env.batch_state.depth == 0;
        if outermost {
            env.write_pending_marker()?;
        }
        env.batch_state.begin();
        if let Err(err) = env.io.begin_batch() {
            env.batch_state.end();
            if outermost {
                let _ = env.remove_pending_marker();
            }
            return Err(err);
        }
        Ok(Self {
            env,
            completed: false,
            outermost,
        })
    }

    fn run<T>(mut self, f: impl FnOnce(&mut OntoEnv) -> Result<T>) -> Result<T> {
        let result = f(self.env);
        let end_result = self.env.io.end_batch().and_then(|_| self.env.io.flush());
        self.env.batch_state.end();
        self.completed = true;
        match (result, end_result) {
            (Ok(value), Ok(())) => {
                if self.outermost {
                    self.env.backend_state = self.env.io.store_state()?;
                    self.env.graph_revisions = self.env.io.graph_revisions()?.unwrap_or_default();
                    self.env.save_to_directory()?;
                    self.env.remove_pending_marker()?;
                }
                Ok(value)
            }
            (Ok(_), Err(err)) => Err(err),
            (Err(err), Ok(())) => Err(err),
            (Err(err), Err(end_err)) => {
                error!("Failed to finalize batched RDF write: {end_err}");
                Err(err)
            }
        }
    }
}

impl<'a> Drop for BatchScope<'a> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        if let Err(err) = self.env.io.end_batch() {
            error!("Failed to finalize batched RDF write: {err}");
        }
        self.env.batch_state.end();
    }
}

enum FetchOutcome {
    Reused(GraphIdentifier),
    Loaded(Box<Ontology>),
}

/// Snapshot of the four mutable fields of [`OntoEnv`] that together form the
/// in-memory ontology environment state. Used by [`OntoEnv::with_env_transaction`]
/// to give callers atomic begin/commit/rollback semantics around operations
/// that can fail partway through (e.g. dependency-graph construction that
/// hits a strict-mode unresolved import after several successful adds).
///
/// Not transactional with respect to `self.io` writes (rdf5d/oxigraph store
/// mutations). A rollback may leave the IO store with orphan named graphs;
/// see the note above [`OntoEnv::add_ids_to_dependency_graph`].
pub(crate) struct EnvTransaction {
    env: Environment,
    dependency_graph: DiGraph<GraphIdentifier, (), petgraph::Directed>,
    dependency_graph_index: HashMap<GraphIdentifier, NodeIndex>,
    failed_resolutions: HashSet<NamedNode>,
}

impl EnvTransaction {
    /// Snapshot the mutable in-memory state of `target` so it can be restored
    /// later via [`Self::restore`].
    pub(crate) fn snapshot(target: &OntoEnv) -> Self {
        Self {
            env: target.env.clone(),
            dependency_graph: target.dependency_graph.clone(),
            dependency_graph_index: target.dependency_graph_index.clone(),
            failed_resolutions: target.failed_resolutions.clone(),
        }
    }

    /// Move the snapshotted state back into `target`, discarding any in-flight
    /// mutations made since [`Self::snapshot`] was called.
    pub(crate) fn restore(self, target: &mut OntoEnv) {
        target.env = self.env;
        target.dependency_graph = self.dependency_graph;
        target.dependency_graph_index = self.dependency_graph_index;
        target.failed_resolutions = self.failed_resolutions;
    }
}

pub struct OntoEnv {
    env: Environment,
    io: Box<dyn GraphIO>,
    dependency_graph: DiGraph<GraphIdentifier, (), petgraph::Directed>,
    /// Maps GraphIdentifier to its NodeIndex in `dependency_graph`. Kept in sync
    /// with the graph so that traversals (closure, importers, paths) avoid a
    /// linear scan of `node_indices()` per lookup.
    dependency_graph_index: HashMap<GraphIdentifier, NodeIndex>,
    config: Config,
    failed_resolutions: HashSet<NamedNode>,
    batch_state: BatchState,
    graph_revisions: HashMap<String, String>,
    backend_state: Option<catalog::BackendState>,
    // Environment-wide lock also protects catalog operations for custom stores.
    _lock_file: Option<File>,
}

impl std::fmt::Debug for OntoEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // print config
        writeln!(f, "OntoEnv {{")?;
        writeln!(f, "  config: {:?},", self.config)?;
        writeln!(f, "  env: {:?},", self.env)?;
        writeln!(f, "  dependency_graph: {:?},", self.dependency_graph)?;
        writeln!(f, "  io: {:?},", self.io.io_type())?;
        write!(f, "}}")?;
        Ok(())
    }
}

fn write_json_file<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    let mut file = File::create(path)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

impl OntoEnv {
    // Constructors
    fn new(env: Environment, io: Box<dyn GraphIO>, config: Config) -> Result<Self> {
        let backend_state = io.store_state().ok().flatten();
        let read_only = io.io_type() == "read-only";
        let lock_file = Self::acquire_environment_lock(&config, read_only)?;
        Ok(Self {
            env,
            io,
            config,
            dependency_graph: DiGraph::new(),
            dependency_graph_index: HashMap::new(),
            failed_resolutions: HashSet::new(),
            batch_state: BatchState::default(),
            graph_revisions: HashMap::new(),
            backend_state,
            _lock_file: lock_file,
        })
    }

    fn acquire_environment_lock(config: &Config, read_only: bool) -> Result<Option<File>> {
        if config.temporary {
            return Ok(None);
        }
        let directory = config.root.join(".ontoenv");
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("store.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        if read_only {
            file.lock_shared()?;
        } else if let Err(error) = file.try_lock_exclusive() {
            return Err(anyhow!(
                "Failed to open OntoEnv for write: could not acquire exclusive lock on {}: {}. If another process has it open, use read_only=true.",
                path.display(),
                error
            ));
        }
        Ok(Some(file))
    }

    /// Create a new empty environment, failing if one already exists unless
    /// `recreate` is explicitly true.
    pub fn create(mut config: Config, recreate: bool) -> Result<Self> {
        config.use_cached_ontologies = crate::options::CacheMode::Enabled;
        Self::init(config, recreate)
    }

    /// Open an existing catalog-backed environment without inspecting
    /// ontology graph contents.
    pub fn open(root: PathBuf, read_only: bool) -> Result<Self> {
        Self::load_from_directory(root, read_only)
    }

    /// Deliberately scan an existing graph backend and publish a new catalog.
    /// This method only reads graphs already exposed by the backend and never
    /// follows network imports.
    pub fn adopt(config: Config, io: Box<dyn GraphIO>) -> Result<Self> {
        let mut environment = Self::new(Environment::new(), io, config)?;
        environment.init_from_graph_io()?;
        environment.backend_state = environment.io.store_state()?;
        environment.graph_revisions = environment.io.graph_revisions()?.unwrap_or_default();
        environment.save_to_directory()?;
        Ok(environment)
    }

    /// Open a catalog while using a caller-provided graph backend.
    pub fn open_with_graph_io(
        config: Config,
        io: Box<dyn GraphIO>,
        _read_only: bool,
    ) -> Result<Self> {
        let catalog_path = config.root.join(".ontoenv").join(catalog::CATALOG_FILE);
        let pending_path = config.root.join(".ontoenv").join(catalog::PENDING_FILE);
        if pending_path.exists() {
            return Err(CatalogRecoveryError {
                message: format!("interrupted mutation marker at {}", pending_path.display()),
            }
            .into());
        }
        if !catalog_path.exists() {
            return Err(anyhow!(
                "OntoEnv catalog not found at {}",
                catalog_path.display()
            ));
        }
        let state_before = io.store_state()?;
        let (mut env, expected, graph_revisions) = catalog::load(&catalog_path)?;
        if let (Some(expected), Some(actual)) = (&expected, &state_before) {
            if expected != actual {
                return Err(ExternalStoreChangedError {
                    message: "backend identity/revision changed; refresh explicitly".to_string(),
                }
                .into());
            }
        }
        if io.store_state()? != state_before {
            return Err(ExternalStoreChangedError {
                message: "backend changed while loading the catalog".to_string(),
            }
            .into());
        }
        env.normalize_file_locations(&config.root);
        let mut result = Self::new(env, io, config)?;
        result.graph_revisions = graph_revisions;
        result.backend_state = expected.or(state_before);
        result.rebuild_dependency_graph_from_metadata();
        Ok(result)
    }

    /// Resolve a path relative to the configured OntoEnv root if it is not already absolute.
    fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            // Prefer current working directory (CLI/Python caller context) so explicit relative
            // search paths like "../brick" behave as users expect, but fall back to root-relative.
            let cwd = std::env::current_dir().unwrap_or_else(|_| self.config.root.clone());
            let cwd_join = cwd.join(path);
            if cwd_join.exists() {
                cwd_join
            } else {
                self.config.root.join(path)
            }
        }
    }

    /// Ensure file locations are anchored to the OntoEnv root and canonicalized;
    /// leave other variants untouched.
    ///
    /// Canonicalization (via [`crate::ontology::canonicalize_file_path`]) is what
    /// makes a path supplied to `add`/`add_from_bytes` compare equal to the path
    /// `init`/`update` discovered for the same file. Without it, symlinked roots
    /// (e.g. macOS `/var` -> `/private/var`) and relative vs absolute forms of
    /// the same file create duplicate ontology entries keyed by location.
    fn resolve_location(&self, location: OntologyLocation) -> OntologyLocation {
        match location {
            OntologyLocation::File(p) => {
                OntologyLocation::File(crate::ontology::canonicalize_file_path(&p))
            }
            _ => location,
        }
    }

    /// Opens an existing environment rooted at `config.root`, or initializes a new one using
    /// the provided configuration when none exists yet.
    pub fn open_or_init(config: Config, read_only: bool) -> Result<Self> {
        // Reuse an existing environment if present; otherwise initialize a new one.
        if config.temporary {
            return Self::init(config, false);
        }

        let root = config.root.clone();

        let existing_root = if let Some(found_root) = find_ontoenv_root_from(&root) {
            if found_root.join(".ontoenv").exists() {
                Some(found_root)
            } else {
                None
            }
        } else if root.join(".ontoenv").exists() {
            Some(root.clone())
        } else {
            None
        };

        if let Some(load_root) = existing_root {
            let mut env = Self::load_from_directory(load_root, read_only)?;
            // The caller provided an explicit config; apply its scalar mode flags
            // (offline, strict, TTL, …) so they take effect even when the env
            // already exists.  List fields (locations, includes, …) from the
            // stored config are left intact.
            if !read_only && env.merge_scalar_flags(&config) {
                env.save_to_directory()?;
            }
            return Ok(env);
        }

        let ontoenv_dir = root.join(".ontoenv");
        if read_only {
            return Err(anyhow::anyhow!(
                "OntoEnv directory not found at {} and read_only=true",
                ontoenv_dir.display()
            ));
        }

        Self::init(config, false)
    }

    /// Creates a new online OntoEnv that searches for ontologies in the current directory.
    /// If an environment already exists, it will be loaded.
    /// The environment will be persisted to disk in the `.ontoenv` directory.
    pub fn new_online() -> Result<Self> {
        // Convenience ctor for local dev: scan cwd and allow network fetches.
        if let Some(root) = find_ontoenv_root() {
            let mut env = Self::load_from_directory(root, false)?;
            if env.is_offline() {
                env.set_offline(false);
                env.save_to_directory()?;
            }
            Ok(env)
        } else {
            let root = std::env::current_dir()?;
            let locations = vec![root.clone()];
            let config = Config::builder()
                .root(root)
                .require_ontology_names(false)
                .strict(false)
                .offline(false)
                .temporary(false)
                .locations(locations)
                .build()?;
            Self::init(config, false)
        }
    }

    /// Creates a new offline OntoEnv that searches for ontologies in the current directory.
    /// If an environment already exists, it will be loaded.
    /// The environment will be persisted to disk in the `.ontoenv` directory.
    pub fn new_offline() -> Result<Self> {
        // Convenience ctor for local dev without network access.
        if let Some(root) = find_ontoenv_root() {
            let mut env = Self::load_from_directory(root, false)?;
            if !env.is_offline() {
                env.set_offline(true);
                env.save_to_directory()?;
            }
            Ok(env)
        } else {
            let root = std::env::current_dir()?;
            let locations = vec![root.clone()];
            let config = Config::builder()
                .root(root)
                .require_ontology_names(false)
                .strict(false)
                .offline(true)
                .temporary(false)
                .locations(locations)
                .build()?;
            Self::init(config, false)
        }
    }

    /// Creates a new offline OntoEnv with no local search paths.
    /// If an environment already exists, it will be loaded.
    /// The environment will be persisted to disk in the `.ontoenv` directory.
    pub fn new_offline_no_search() -> Result<Self> {
        // Offline mode with no search paths to avoid filesystem scans.
        if let Some(root) = find_ontoenv_root() {
            let mut env = Self::load_from_directory(root, false)?;
            if !env.is_offline() {
                env.set_offline(true);
                env.save_to_directory()?;
            }
            Ok(env)
        } else {
            let root = std::env::current_dir()?;
            let config = Config::builder()
                .root(root)
                .require_ontology_names(false)
                .strict(false)
                .offline(true)
                .temporary(false)
                .locations(vec![])
                .build()?;
            Self::init(config, false)
        }
    }

    /// Creates a new online, in-memory OntoEnv with no local search paths.
    /// This is useful for working with remote ontologies only.
    pub fn new_in_memory_online_no_search() -> Result<Self> {
        // Ephemeral environment for remote-only workflows.
        let root = std::env::current_dir()?; // root is still needed for config
        let config = Config::builder()
            .root(root)
            .require_ontology_names(false)
            .strict(false)
            .offline(false)
            .temporary(true)
            .locations(vec![])
            .build()?;
        Self::init(config, true) // overwrite is fine for in-memory
    }

    /// Creates a new online, in-memory OntoEnv that searches for ontologies in the current directory.
    pub fn new_in_memory_online_with_search() -> Result<Self> {
        // Ephemeral environment that still scans the current directory.
        let root = std::env::current_dir()?;
        let locations = vec![root.clone()];
        let config = Config::builder()
            .root(root)
            .require_ontology_names(false)
            .strict(false)
            .offline(false)
            .temporary(true)
            .locations(locations)
            .build()?;
        Self::init(config, true)
    }

    pub fn new_from_store(strict: bool, offline: bool, store: Store) -> Result<Self> {
        // Wrap an existing Oxigraph store for embedding into other applications.
        let io = Box::new(crate::io::ExternalStoreGraphIO::new(store, offline, strict));
        let root = std::env::current_dir()?;
        let locations = vec![root.clone()];
        let config = Config::builder()
            .root(root)
            .require_ontology_names(false)
            .strict(strict)
            .offline(offline)
            .temporary(false)
            .locations(locations)
            .build()?;

        let mut ontoenv = Self::new(Environment::new(), io, config)?;
        let _ = ontoenv.update_all(false)?;
        Ok(ontoenv)
    }

    /// Creates a new OntoEnv using a caller-provided GraphIO implementation.
    /// This is useful for embedding OntoEnv into applications with custom graph storage.
    pub fn new_with_graph_io(config: Config, io: Box<dyn GraphIO>) -> Result<Self> {
        // Plug in a custom GraphIO implementation and follow the same
        // cache/bootstrap behavior as the built-in backend.
        let mut ontoenv = Self::new(Environment::new(), io, config)?;
        if !ontoenv.config.use_cached_ontologies.is_enabled() {
            let _ = ontoenv.update_all(false)?;
        }
        ontoenv.backend_state = ontoenv.io.store_state()?;
        ontoenv.graph_revisions = ontoenv.io.graph_revisions()?.unwrap_or_default();
        ontoenv.save_to_directory()?;
        Ok(ontoenv)
    }

    /// Creates a new OntoEnv by reading the graphs already present in a caller-provided
    /// GraphIO store.  Ontology metadata and the import dependency graph are reconstructed
    /// from the store contents; no filesystem discovery or network fetching is performed.
    pub fn new_with_graph_io_from_existing(config: Config, io: Box<dyn GraphIO>) -> Result<Self> {
        Self::adopt(config, io)
    }

    /// Re-synchronizes the environment with the current contents of the underlying GraphIO
    /// store.  Use this when the store has been mutated externally and the in-memory
    /// environment state (ontology metadata, import graph) needs to be brought up to date.
    pub fn refresh_from_graph_io(&mut self) -> Result<()> {
        self.refresh_from_store(None, true).map(|_| ())
    }

    /// Explicitly reconcile catalog metadata with the graph backend.
    pub fn refresh_from_store(
        &mut self,
        graphs: Option<Vec<String>>,
        full: bool,
    ) -> Result<SyncReport> {
        if full && graphs.is_some() {
            return Err(anyhow!("graphs cannot be combined with full=true"));
        }

        let old: HashMap<String, Option<String>> = self
            .env
            .ontologies()
            .values()
            .map(|ontology| {
                (
                    ontology.id().name().as_str().to_string(),
                    ontology.content_hash().map(str::to_string),
                )
            })
            .collect();

        if full {
            self.env = Environment::new();
            self.init_from_graph_io()?;
            let current: HashSet<_> = self
                .env
                .ontologies()
                .keys()
                .map(|id| id.name().as_str().to_string())
                .collect();
            let mut report = SyncReport {
                mode: SyncMode::Full,
                added: current
                    .iter()
                    .filter(|id| !old.contains_key(*id))
                    .cloned()
                    .collect(),
                changed: current
                    .iter()
                    .filter(|id| old.contains_key(*id))
                    .cloned()
                    .collect(),
                removed: old
                    .keys()
                    .filter(|id| !current.contains(*id))
                    .cloned()
                    .collect(),
                unchanged: Vec::new(),
                still_pending: Vec::new(),
            };
            report.added.sort();
            report.changed.sort();
            report.removed.sort();
            self.backend_state = self.io.store_state()?;
            self.graph_revisions = self.io.graph_revisions()?.unwrap_or_default();
            self.save_to_directory()?;
            self.remove_pending_marker()?;
            return Ok(report);
        }

        let current_revisions = self.io.graph_revisions()?;
        let (mode, targets) = if let Some(graphs) = graphs {
            (SyncMode::Targeted, graphs)
        } else {
            let Some(revisions) = current_revisions.as_ref() else {
                // Temporary environments have no authoritative catalog to
                // protect, so retain the historical explicit rescan behavior.
                if self.config.temporary {
                    return self.refresh_from_store(None, true);
                }
                return Err(StoreCapabilityError {
                    message:
                        "graph backend does not implement graph_revisions(); use graphs=[...] or full=true"
                            .to_string(),
                }
                .into());
            };
            let mut targets: HashSet<String> = revisions
                .iter()
                .filter(|(id, revision)| self.graph_revisions.get(*id) != Some(*revision))
                .map(|(id, _)| id.clone())
                .collect();
            targets.extend(
                self.graph_revisions
                    .keys()
                    .filter(|id| !revisions.contains_key(*id))
                    .cloned(),
            );
            if targets.is_empty() {
                let mut unchanged: Vec<_> = revisions.keys().cloned().collect();
                unchanged.sort();
                return Ok(SyncReport {
                    mode: SyncMode::Incremental,
                    added: Vec::new(),
                    changed: Vec::new(),
                    removed: Vec::new(),
                    unchanged,
                    still_pending: Vec::new(),
                });
            }
            (SyncMode::Incremental, targets.into_iter().collect())
        };

        let backend_ids: HashMap<String, GraphIdentifier> = self
            .io
            .graph_ids()?
            .into_iter()
            .map(|id| (id.name().as_str().to_string(), id))
            .collect();
        let mut added = Vec::new();
        let mut changed = Vec::new();
        let mut removed = Vec::new();
        let mut unchanged = Vec::new();
        for graph in &targets {
            let existing = self
                .env
                .ontologies()
                .keys()
                .find(|id| id.name().as_str() == graph)
                .cloned();
            let Some(backend_id) = backend_ids.get(graph) else {
                if let Some(existing) = existing {
                    self.env.remove_ontology(&existing)?;
                    removed.push(graph.clone());
                } else {
                    unchanged.push(graph.clone());
                }
                self.graph_revisions.remove(graph);
                continue;
            };
            let id = existing
                .as_ref()
                .map(|known| {
                    GraphIdentifier::new_with_location(backend_id.name(), known.location().clone())
                })
                .unwrap_or_else(|| backend_id.clone());
            let ontology = self.ontology_metadata_from_backend(&id)?;
            if let Some(existing) = existing {
                self.env.remove_ontology(&existing)?;
                changed.push(graph.clone());
            } else {
                added.push(graph.clone());
            }
            self.env.add_ontology(ontology)?;
            if let Some(revisions) = &current_revisions {
                if let Some(revision) = revisions.get(graph) {
                    self.graph_revisions.insert(graph.clone(), revision.clone());
                }
            }
        }
        self.rebuild_dependency_graph_from_metadata();

        let mut still_pending = Vec::new();
        if let Some(revisions) = &current_revisions {
            still_pending.extend(
                revisions
                    .iter()
                    .filter(|(id, revision)| self.graph_revisions.get(*id) != Some(*revision))
                    .map(|(id, _)| id.clone()),
            );
            still_pending.extend(
                self.graph_revisions
                    .keys()
                    .filter(|id| !revisions.contains_key(*id))
                    .cloned(),
            );
            still_pending.sort();
            still_pending.dedup();
        }
        if still_pending.is_empty() {
            self.backend_state = self.io.store_state()?;
        }
        self.save_to_directory()?;
        added.sort();
        changed.sort();
        removed.sort();
        unchanged.sort();
        Ok(SyncReport {
            mode,
            added,
            changed,
            removed,
            unchanged,
            still_pending,
        })
    }

    fn ontology_metadata_from_backend(&self, id: &GraphIdentifier) -> Result<Ontology> {
        let graph = self.io.get_graph(id)?;
        let tmp_store = Store::new()?;
        let graphname = id.graphname()?;
        let quads = graph.iter().map(|triple| {
            Ok::<_, oxigraph::store::StorageError>(Quad::new(
                triple.subject.into_owned(),
                triple.predicate.into_owned(),
                triple.object.into_owned(),
                graphname.clone(),
            ))
        });
        let mut loader = tmp_store.bulk_loader();
        loader.load_ok_quads::<_, oxigraph::store::StorageError>(quads)?;
        loader
            .commit()
            .map_err(|error| anyhow!(error.to_string()))?;
        Ontology::from_store(&tmp_store, id, self.config.strict)
    }

    /// Reads all graphs from the IO layer, derives `Ontology` metadata from each one, and
    /// registers them in the environment together with a freshly-built dependency graph.
    fn init_from_graph_io(&mut self) -> Result<()> {
        let ids = self.io.graph_ids()?;
        if ids.is_empty() {
            return Ok(());
        }
        let strict = self.config.strict;
        let mut ontologies = Vec::with_capacity(ids.len());
        for id in &ids {
            let graph = match self.io.get_graph(id) {
                Ok(g) => g,
                Err(e) => {
                    warn!("init_from_graph_io: could not read graph {id}: {e}");
                    continue;
                }
            };
            // Copy the graph's triples into a temporary store under the correct named graph
            // so that Ontology::from_store can locate the right graph context.
            let tmp_store = Store::new()?;
            let graphname = id.graphname()?;
            let quads = graph.iter().map(|t| {
                Ok::<_, oxigraph::store::StorageError>(Quad::new(
                    t.subject.into_owned(),
                    t.predicate.into_owned(),
                    t.object.into_owned(),
                    graphname.clone(),
                ))
            });
            let mut loader = tmp_store.bulk_loader();
            loader.load_ok_quads::<_, oxigraph::store::StorageError>(quads)?;
            loader.commit().map_err(|e| anyhow!(e.to_string()))?;
            match Ontology::from_store(&tmp_store, id, strict) {
                Ok(ont) => ontologies.push(ont),
                Err(e) => warn!("init_from_graph_io: could not parse ontology from {id}: {e}"),
            }
        }
        let filters = self.ontology_filters()?;
        for ontology in ontologies {
            if filters.allow(ontology.id()) {
                self.env.add_ontology(ontology)?;
            }
        }
        self.rebuild_dependency_graph_from_metadata();
        Ok(())
    }

    /// returns the graph identifier for the given resolve target, if it exists
    pub fn resolve(&self, target: ResolveTarget) -> Option<GraphIdentifier> {
        // Map a location or graph IRI to the canonical GraphIdentifier.
        match target {
            ResolveTarget::Location(location) => {
                // Canonicalize file locations so callers can pass paths obtained
                // from `tempfile` (e.g. macOS `/var` -> `/private/var` symlink)
                // or other non-canonical forms and still match the canonical
                // keys recorded by `init`/`update`/`add`.
                let location = self.resolve_location(location);
                self.env
                    .get_ontology_by_location(&location)
                    .map(|ont| ont.id().clone())
            }
            ResolveTarget::Graph(iri) => self
                .env
                .get_ontology_by_name(iri.as_ref())
                .map(|ont| ont.id().clone()),
        }
    }

    /// Saves the current environment to the .ontoenv directory.
    pub fn save_to_directory(&self) -> Result<()> {
        if self.config.temporary {
            warn!("Cannot save a temporary environment");
            return Ok(());
        }
        let ontoenv_dir = self.config.root.join(".ontoenv");
        info!("Saving ontology environment to: {ontoenv_dir:?}");
        std::fs::create_dir_all(&ontoenv_dir)?;

        write_json_file(&ontoenv_dir.join("ontoenv.json"), &self.config)?;
        catalog::save(
            &ontoenv_dir.join(catalog::CATALOG_FILE),
            &self.env,
            self.backend_state.clone(),
            self.graph_revisions.clone(),
        )?;

        Ok(())
    }

    fn write_pending_marker(&self) -> Result<()> {
        if self.config.temporary {
            return Ok(());
        }
        let directory = self.config.root.join(".ontoenv");
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(catalog::PENDING_FILE);
        let graphs: Vec<_> = self
            .env
            .ontologies()
            .keys()
            .map(|id| id.name().as_str().to_string())
            .collect();
        let marker = serde_json::json!({
            "mutation_id": format!("{}-{}", std::process::id(), Utc::now().timestamp_nanos_opt().unwrap_or_default()),
            "graphs": graphs,
        });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)?;
        file.write_all(serde_json::to_string(&marker)?.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    }

    fn remove_pending_marker(&self) -> Result<()> {
        if self.config.temporary {
            return Ok(());
        }
        let path = self
            .config
            .root
            .join(".ontoenv")
            .join(catalog::PENDING_FILE);
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn new_temporary(&self) -> Result<Self> {
        // Clone the environment into an in-memory store for safe experimentation.
        let io: Box<dyn GraphIO> = Box::new(crate::io::MemoryGraphIO::new(
            self.config.offline,
            self.config.strict,
        )?);
        let mut config = self.config.clone();
        config.temporary = true;
        Self::new(self.env.clone(), io, config)
    }

    fn ontology_filters(&self) -> Result<OntologyFilters> {
        let (include, exclude) = self.config.build_ontology_regexes()?;
        Ok(OntologyFilters { include, exclude })
    }

    fn prune_disallowed_ontologies(
        &mut self,
        filters: &OntologyFilters,
        touch_io: bool,
    ) -> Result<()> {
        let mut removed = Vec::new();
        for id in self.env.ontologies().keys().cloned().collect::<Vec<_>>() {
            if filters.allow(&id) {
                continue;
            }
            info!("Excluding ontology {} due to ontology filters", id);
            if touch_io {
                if let Err(err) = self.io.remove(&id) {
                    warn!(
                        "Failed to remove filtered ontology {} from store: {}",
                        id, err
                    );
                }
            }
            let _ = self.env.remove_ontology(&id)?;
            removed.push(id);
        }

        if !removed.is_empty() {
            // Dependency graph may contain stale nodes; rebuild to stay consistent.
            self.rebuild_dependency_graph()?;
        }
        Ok(())
    }

    /// Loads the environment from the .ontoenv directory.
    pub fn load_from_directory(root: PathBuf, read_only: bool) -> Result<Self> {
        // Load configuration and the authoritative metadata catalog. Ontology
        // graph contents are deliberately not touched on this path.
        let ontoenv_dir = root.join(".ontoenv");
        if !ontoenv_dir.exists() {
            return Err(anyhow::anyhow!(
                "OntoEnv directory not found at: {:?}",
                ontoenv_dir
            ));
        }

        // Load the environment configuration
        let config_path = ontoenv_dir.join("ontoenv.json");
        let file = std::fs::File::open(config_path)?;
        let reader = BufReader::new(file);
        let config: Config = serde_json::from_reader(reader)?;
        if let Some(store) = &config.external_graph_store {
            warn!(
                "OntoEnv uses an external graph store ({store}). The CLI cannot access that store; use the Python bindings instead."
            );
        }

        let pending_path = ontoenv_dir.join(catalog::PENDING_FILE);
        if pending_path.exists() {
            let details = std::fs::read_to_string(&pending_path).unwrap_or_default();
            return Err(CatalogRecoveryError {
                message: format!(
                    "{} records an interrupted backend/catalog mutation ({}). Run refresh_from_store(graphs=[...]) for the listed graphs or refresh_from_store(full=true)",
                    pending_path.display(),
                    details
                ),
            }
            .into());
        }

        let lock_file = Self::acquire_environment_lock(&config, read_only)?;
        let io: Box<dyn GraphIO> = match read_only {
            true => Box::new(crate::io::ReadOnlyPersistentGraphIO::new(
                ontoenv_dir.clone(),
                config.offline,
            )?),
            false => Box::new(crate::io::PersistentGraphIO::new(
                ontoenv_dir.clone(),
                config.offline,
                config.strict,
            )?),
        };
        let state_before = io.store_state()?;

        let catalog_path = ontoenv_dir.join(catalog::CATALOG_FILE);
        let (mut env, expected_state, graph_revisions, migrated) = if catalog_path.exists() {
            let (env, state, revisions) = catalog::load(&catalog_path)?;
            (env, state, revisions, false)
        } else {
            let legacy_path = ontoenv_dir.join("environment.json");
            if !legacy_path.exists() {
                return Err(anyhow!(
                    "OntoEnv catalog not found at {}",
                    catalog_path.display()
                ));
            }
            let file = std::fs::File::open(&legacy_path)?;
            let reader = BufReader::new(file);
            let env: Environment = serde_json::from_reader(reader)?;
            let metadata_ids: HashSet<_> = env
                .ontologies()
                .keys()
                .map(|id| id.name().as_str().to_string())
                .collect();
            let backend_ids: HashSet<_> = io
                .graph_ids()?
                .into_iter()
                .map(|id| id.name().as_str().to_string())
                .collect();
            if metadata_ids != backend_ids {
                return Err(anyhow!(
                    "legacy metadata does not match backend graph IDs; use a targeted refresh or refresh_from_store(full=true)"
                ));
            }
            (env, state_before.clone(), HashMap::new(), true)
        };
        env.normalize_file_locations(&config.root);
        if let (Some(expected), Some(actual)) = (&expected_state, &state_before) {
            if expected != actual {
                return Err(ExternalStoreChangedError {
                    message: format!(
                        "backend identity/revision changed (catalog {:?}, backend {:?}); refresh explicitly",
                        expected, actual
                    ),
                }
                .into());
            }
        }
        let state_after = io.store_state()?;
        if state_before != state_after {
            return Err(ExternalStoreChangedError {
                message: "backend changed while the catalog was being loaded".to_string(),
            }
            .into());
        }

        let mut ontoenv = OntoEnv {
            env,
            io,
            config,
            dependency_graph: DiGraph::new(),
            dependency_graph_index: HashMap::new(),
            failed_resolutions: HashSet::new(),
            batch_state: BatchState::default(),
            graph_revisions,
            backend_state: expected_state.or(state_before),
            _lock_file: lock_file,
        };
        ontoenv.rebuild_dependency_graph_from_metadata();
        if migrated && !read_only {
            ontoenv.save_to_directory()?;
        }

        let filters = ontoenv.ontology_filters()?;
        // Avoid writing when read_only; prune in-memory only for read-only or temporary envs.
        let touch_io = !(read_only || ontoenv.config.temporary);
        ontoenv.prune_disallowed_ontologies(&filters, touch_io)?;

        Ok(ontoenv)
    }

    // Core API methods
    pub fn flush(&mut self) -> Result<()> {
        // Force pending writes to the underlying store implementation.
        self.io.flush()
    }

    fn with_io_batch<T, F>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Self) -> Result<T>,
    {
        BatchScope::enter(self)?.run(f)
    }

    pub fn io(&self) -> &dyn GraphIO {
        // Expose the IO backend for advanced integrations.
        self.io.as_ref()
    }

    pub fn stats(&self) -> Result<Stats> {
        // Aggregate store and environment counts for quick diagnostics.
        let store_stats = self.io.size()?;
        Ok(Stats {
            num_triples: store_stats.num_triples,
            num_graphs: store_stats.num_graphs,
            num_ontologies: self.env.ontologies().len(),
        })
    }

    /// Backwards-compatibility: update only changed/added files (same as update_all(false))
    pub fn update(&mut self) -> Result<Vec<GraphIdentifier>> {
        // Preserve legacy API while delegating to update_all(false).
        self.update_all(false)
    }

    /// Calculates and returns the environment status
    pub fn status(&self) -> Result<EnvironmentStatus> {
        let ontoenv_dir = self.config.root.join(".ontoenv");
        let ontoenv_path = fs::canonicalize(&ontoenv_dir).unwrap_or_else(|_| ontoenv_dir.clone());
        let last_updated: DateTime<Utc> = std::fs::metadata(&ontoenv_dir)?.modified()?.into();
        // get the size of the .ontoenv directory on disk
        let size: u64 = walkdir::WalkDir::new(ontoenv_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        let num_ontologies = self.env.ontologies().len();
        let missing_imports = self.missing_imports();
        Ok(EnvironmentStatus {
            exists: true,
            ontoenv_path: Some(ontoenv_path),
            num_ontologies,
            last_updated: Some(last_updated),
            store_size: size,
            missing_imports,
        })
    }

    pub fn store_path(&self) -> Option<&Path> {
        // Return the store location if this IO backend is persistent.
        self.io.store_location()
    }

    pub fn ontologies(&self) -> &HashMap<GraphIdentifier, Ontology> {
        // Expose the environment's ontology map for read-only inspection.
        self.env.ontologies()
    }

    /// Returns a table of metadata for the given graph
    pub fn graph_metadata(&self, id: &GraphIdentifier) -> HashMap<String, String> {
        // Build a simple string map for CLI display and JSON outputs.
        let mut metadata = HashMap::new();
        if let Some(ontology) = self.ontologies().get(id) {
            metadata.insert("name".to_string(), ontology.name().to_string());
            metadata.insert(
                "location".to_string(),
                ontology
                    .location()
                    .map_or("".to_string(), |loc| loc.to_string()),
            );
            if let Some(last_updated) = ontology.last_updated {
                metadata.insert("last_updated".to_string(), last_updated.to_string());
            }
            // add all metadata from the graph ontology object
            for (key, value) in ontology.version_properties().iter() {
                metadata.insert(key.to_string(), value.to_string());
            }
        }
        metadata
    }

    /// Initializes a new API environment based on `config`.
    ///
    /// For persistent environments (`config.temporary == false`), if the target `.ontoenv`
    /// directory already exists this will remove and recreate it when `overwrite` is `true`,
    /// otherwise it returns an error. Temporary environments never touch the filesystem, so
    /// the `overwrite` flag is ignored. When the cache mode is disabled the initializer runs
    /// a discovery pass so the store eagerly reflects on-disk content; when the cache mode is
    /// enabled the environment starts empty and only fetches when explicitly asked to.
    pub fn init(config: Config, overwrite: bool) -> Result<Self> {
        // Create a fresh environment, optionally overwriting existing data on disk.
        let ontoenv_dir = config.root.join(".ontoenv");

        if !config.temporary && ontoenv_dir.exists() {
            if overwrite {
                info!("Directory exists and will be overwritten: {ontoenv_dir:?}");
                fs::remove_dir_all(&ontoenv_dir)?;
            } else {
                return Err(anyhow::anyhow!(
                    "Directory already exists: {:?}. Use '--overwrite' to force reinitialization.",
                    ontoenv_dir
                ));
            }
        }

        if !config.temporary {
            std::fs::create_dir_all(&ontoenv_dir)?;
        }
        let lock_file = Self::acquire_environment_lock(&config, false)?;

        let env = Environment::new();
        let io: Box<dyn GraphIO> = match config.temporary {
            true => Box::new(crate::io::MemoryGraphIO::new(
                config.offline,
                config.strict,
            )?),
            false => Box::new(crate::io::PersistentGraphIO::new(
                ontoenv_dir,
                config.offline,
                config.strict,
            )?),
        };

        let mut ontoenv = OntoEnv {
            env,
            io,
            dependency_graph: DiGraph::new(),
            dependency_graph_index: HashMap::new(),
            config,
            failed_resolutions: HashSet::new(),
            batch_state: BatchState::default(),
            graph_revisions: HashMap::new(),
            backend_state: None,
            _lock_file: lock_file,
        };

        if !ontoenv.config.use_cached_ontologies.is_enabled() {
            let _ = ontoenv.update_all(false)?;
        }
        ontoenv.backend_state = ontoenv.io.store_state()?;
        ontoenv.graph_revisions = ontoenv.io.graph_revisions()?.unwrap_or_default();

        // Always persist the config so flags like `offline` survive across sessions.
        // `update_all` writes via `register_ontologies`, but that path is skipped when
        // `use_cached_ontologies` is enabled or when no ontologies are discovered, so
        // we call it explicitly here as the authoritative write.
        ontoenv.save_to_directory()?;

        Ok(ontoenv)
    }

    /// Deletes the .ontoenv directory, searching from the current directory upwards.
    pub fn reset() -> Result<()> {
        // Remove the nearest .ontoenv directory if one exists.
        if let Some(root) = find_ontoenv_root() {
            let ontoenv_dir = root.join(".ontoenv");
            info!("Removing ontology environment at: {ontoenv_dir:?}");
            if ontoenv_dir.exists() {
                std::fs::remove_dir_all(&ontoenv_dir)?;
            }
        }
        Ok(())
    }

    /// Add the ontology from the given location to the environment,
    /// then add it to the dependency graph.
    ///
    /// * `overwrite` selects whether an existing graph at the same identifier should be replaced.
    /// * `refresh` controls whether cached metadata may be reused (`RefreshStrategy::UseCache`) or
    ///   the source should always be fetched (`RefreshStrategy::Force`).
    pub fn add(
        &mut self,
        location: OntologyLocation,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
    ) -> Result<GraphIdentifier> {
        // Default add behavior: include imports and update dependency graph.
        self.add_with_options(location, overwrite, refresh, true)
    }

    /// Add the ontology from the given location to the environment, but do not
    /// explore its owl:imports. It will be added to the dependency graph and
    /// edges will be created if its imports are already present in the environment.
    /// Parameters mirror [`OntoEnv::add`] for overwrite and refresh behavior.
    pub fn add_no_imports(
        &mut self,
        location: OntologyLocation,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
    ) -> Result<GraphIdentifier> {
        // Add a single ontology without traversing its imports.
        self.add_with_options(location, overwrite, refresh, false)
    }

    /// Add the ontology from the given location, rename its declared IRI to `rename`, then
    /// traverse its `owl:imports`.  All occurrences of the original IRI inside the graph
    /// (subject and object positions) are rewritten to `rename` before the graph is stored.
    pub fn add_with_rename(
        &mut self,
        location: OntologyLocation,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
        rename: NamedNode,
    ) -> Result<GraphIdentifier> {
        let old_id = self.add(location, overwrite, refresh)?;
        self.apply_graph_rename(&old_id, rename)
    }

    /// Like [`Self::add_with_rename`] but does not traverse `owl:imports`.
    pub fn add_no_imports_with_rename(
        &mut self,
        location: OntologyLocation,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
        rename: NamedNode,
    ) -> Result<GraphIdentifier> {
        let old_id = self.add_no_imports(location, overwrite, refresh)?;
        self.apply_graph_rename(&old_id, rename)
    }

    /// Rename the graph IRI of an already-loaded ontology.
    ///
    /// Reads the graph from the IO store, rewrites every occurrence of the old IRI to
    /// `new_iri` (subject and object positions), writes it back under the new name,
    /// removes the old named graph, and updates the in-memory environment and dependency
    /// graph to reflect the change.
    pub fn rename_graph_iri(
        &mut self,
        id: &GraphIdentifier,
        new_iri: NamedNode,
    ) -> Result<GraphIdentifier> {
        self.apply_graph_rename(id, new_iri)
    }

    fn apply_graph_rename(
        &mut self,
        old_id: &GraphIdentifier,
        new_iri: NamedNode,
    ) -> Result<GraphIdentifier> {
        let old_id = old_id.clone();
        self.with_io_batch(move |environment| {
            environment.apply_graph_rename_inner(&old_id, new_iri)
        })
    }

    fn apply_graph_rename_inner(
        &mut self,
        old_id: &GraphIdentifier,
        new_iri: NamedNode,
    ) -> Result<GraphIdentifier> {
        if old_id.name() == new_iri.as_ref() {
            return Ok(old_id.clone());
        }

        // Read graph, apply rename transform, write back under new name.
        let mut graph = self.io.get_graph(old_id)?;
        transform::rename_ontology_iri_graph(&mut graph, old_id.name(), new_iri.as_ref());
        let new_id =
            GraphIdentifier::new_with_location(new_iri.as_ref(), old_id.location().clone());
        self.io.remove(old_id)?;
        self.io.add_named_graph(new_id.clone(), graph)?;

        // Update in-memory environment: remove old entry, re-insert under new IRI.
        if let Some(mut ont) = self.env.remove_ontology(old_id)? {
            ont.set_iri(new_iri);
            self.env.add_ontology(ont)?;
        }

        // Rebuild so dependency graph edges reflect the new identifier.
        self.rebuild_dependency_graph_from_metadata();
        Ok(new_id)
    }

    /// Add an alias for a canonical ontology IRI.
    ///
    /// The alias will route to the same graph as the canonical IRI.
    /// Aliases only point to canonical IRIs (not other aliases) to avoid chains.
    pub fn add_alias(&mut self, alias_iri: &str, canonical_iri: &str) -> Result<()> {
        self.env.add_alias(alias_iri, canonical_iri)?;
        self.save_to_directory()?;
        Ok(())
    }

    /// Remove an alias.
    pub fn remove_alias(&mut self, alias_iri: &str) -> Result<()> {
        if self.env.remove_alias(alias_iri)?.is_some() {
            self.save_to_directory()?;
        }
        Ok(())
    }

    /// Get the canonical GraphIdentifier for an alias.
    ///
    /// Returns None if the IRI is not an alias.
    pub fn resolve_alias(&self, alias_iri: &str) -> Option<GraphIdentifier> {
        let alias_norm = Environment::normalize_name(alias_iri).to_string();
        self.env.aliases().get(&alias_norm).cloned()
    }

    /// List all aliases that point to a given canonical IRI.
    pub fn get_aliases_for(&self, canonical_iri: &str) -> Vec<String> {
        self.env.get_aliases_for(canonical_iri)
    }

    /// Check if an IRI is a canonical ontology (not an alias).
    pub fn is_canonical_iri(&self, iri: &str) -> bool {
        self.env.is_canonical_iri(iri)
    }

    /// Add an ontology from in-memory bytes and traverse its imports.
    ///
    /// The root ontology is parsed from `bytes` and associated with `location` without creating
    /// a temporary file. Imported ontologies are still resolved from their declared permanent
    /// locations (`http(s)`/`file`) using the same strict/offline/filter behavior as [`Self::add`].
    pub fn add_from_bytes(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
    ) -> Result<GraphIdentifier> {
        self.add_from_bytes_with_options(location, bytes, format, overwrite, refresh, true, None)
    }

    /// Add an ontology from in-memory bytes without traversing imports.
    ///
    /// Mirrors [`Self::add_no_imports`] for import traversal behavior while keeping the root
    /// ontology content in memory.
    pub fn add_from_bytes_no_imports(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
    ) -> Result<GraphIdentifier> {
        self.add_from_bytes_with_options(location, bytes, format, overwrite, refresh, false, None)
    }

    /// Add an ontology from in-memory bytes with optional import traversal depth.
    ///
    /// `max_import_depth = None` means unbounded traversal (same as [`Self::add_from_bytes`]).
    /// `Some(0)` loads only the root ontology bytes.
    pub fn add_from_bytes_with_import_depth(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
        max_import_depth: Option<usize>,
    ) -> Result<GraphIdentifier> {
        self.add_from_bytes_with_options(
            location,
            bytes,
            format,
            overwrite,
            refresh,
            true,
            max_import_depth,
        )
    }

    fn add_with_options(
        &mut self,
        location: OntologyLocation,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
        update_dependencies: bool,
    ) -> Result<GraphIdentifier> {
        let location = self.resolve_location(location);
        self.with_io_batch(move |env| {
            env.add_with_options_inner(location, overwrite, refresh, update_dependencies, None)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn add_from_bytes_with_options(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
        update_dependencies: bool,
        max_import_depth: Option<usize>,
    ) -> Result<GraphIdentifier> {
        let location = self.resolve_location(location);
        self.with_io_batch(move |env| {
            env.add_from_bytes_with_options_inner(
                location,
                bytes,
                format,
                overwrite,
                refresh,
                update_dependencies,
                max_import_depth,
            )
        })
    }

    fn fetch_location(
        &mut self,
        job: PendingImport,
        refresh: RefreshStrategy,
    ) -> Result<FetchOutcome> {
        match job {
            PendingImport::FromBytes {
                location,
                overwrite,
                bytes,
                format,
                ..
            } => {
                if let Some(existing_id) =
                    self.try_reuse_cached_bytes(&location, bytes.as_slice(), refresh)?
                {
                    self.batch_state.mark_seen(&location);
                    return Ok(FetchOutcome::Reused(existing_id));
                }

                if let Some(existing_id) = self.reuse_if_seen_in_batch(&location, refresh) {
                    return Ok(FetchOutcome::Reused(existing_id));
                }

                let ontology =
                    self.io
                        .add_from_bytes(location.clone(), bytes, format, overwrite)?;
                self.batch_state.mark_seen(&location);
                Ok(FetchOutcome::Loaded(Box::new(ontology)))
            }
            PendingImport::FromLocation {
                location,
                overwrite,
                ..
            } => {
                if let Some(existing_id) = self.try_reuse_cached(&location, refresh)? {
                    self.batch_state.mark_seen(&location);
                    return Ok(FetchOutcome::Reused(existing_id));
                }

                if let Some(existing_id) = self.reuse_if_seen_in_batch(&location, refresh) {
                    return Ok(FetchOutcome::Reused(existing_id));
                }

                let ontology = self.io.add(location.clone(), overwrite)?;
                self.batch_state.mark_seen(&location);
                Ok(FetchOutcome::Loaded(Box::new(ontology)))
            }
        }
    }

    fn reuse_if_seen_in_batch(
        &self,
        location: &OntologyLocation,
        refresh: RefreshStrategy,
    ) -> Option<GraphIdentifier> {
        if refresh.is_force() || !self.batch_state.has_seen(location) {
            return None;
        }
        self.env
            .get_ontology_by_location(location)
            .map(|existing| existing.id().clone())
    }

    fn register_ontologies(
        &mut self,
        ontologies: Vec<Ontology>,
        update_dependencies: bool,
        filters: &OntologyFilters,
    ) -> Result<Vec<GraphIdentifier>> {
        let mut ids = Vec::with_capacity(ontologies.len());
        for ontology in ontologies {
            let id = ontology.id().clone();
            if !filters.allow(&id) {
                info!("Excluding ontology {} due to ontology filters", id);
                continue;
            }
            self.env.add_ontology(ontology)?;
            ids.push(id);
        }

        if update_dependencies && !ids.is_empty() {
            self.add_ids_to_dependency_graph(ids.clone())?;
        }

        Ok(ids)
    }
    fn add_with_options_inner(
        &mut self,
        location: OntologyLocation,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
        update_dependencies: bool,
        max_import_depth: Option<usize>,
    ) -> Result<GraphIdentifier> {
        let seed = PendingImport::from_location(location, overwrite, self.config.strict, 0);
        self.add_with_seed_inner(seed, refresh, update_dependencies, max_import_depth)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_from_bytes_with_options_inner(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
        update_dependencies: bool,
        max_import_depth: Option<usize>,
    ) -> Result<GraphIdentifier> {
        let seed =
            PendingImport::from_bytes(location, overwrite, self.config.strict, 0, bytes, format);
        self.add_with_seed_inner(seed, refresh, update_dependencies, max_import_depth)
    }

    fn add_with_seed_inner(
        &mut self,
        seed: PendingImport,
        refresh: RefreshStrategy,
        update_dependencies: bool,
        max_import_depth: Option<usize>,
    ) -> Result<GraphIdentifier> {
        let (location_ref, _, _) = seed.meta();
        let location = location_ref.clone();
        // Reset per-call error tracking so stale failures do not leak across operations.
        self.failed_resolutions.clear();
        // Apply ontology filters early to keep store and env consistent.
        let ontology_filters = self.ontology_filters()?;
        self.prune_disallowed_ontologies(&ontology_filters, true)?;
        // Seed the import queue with the requested location and overwrite policy.
        let seeds = vec![seed];
        let (ontologies, reused_ids, errors) = self.process_import_queue(
            seeds,
            refresh,
            update_dependencies,
            max_import_depth,
            &mut ProgressReporter::silent(),
        )?;
        // Filter newly fetched ontologies before registering them.
        let filtered_onts: Vec<Ontology> = ontologies
            .into_iter()
            .filter(|o| ontology_filters.allow(o.id()))
            .collect();
        let mut ids =
            self.register_ontologies(filtered_onts, update_dependencies, &ontology_filters)?;
        // Include cached/reused identifiers that still pass filters.
        ids.extend(
            reused_ids
                .into_iter()
                .filter(|id| ontology_filters.allow(id)),
        );

        // Prefer the ontology at the requested location when present.
        if let Some(existing) = self.env.get_ontology_by_location(&location) {
            if ontology_filters.allow(existing.id()) {
                return Ok(existing.id().clone());
            } else {
                return Err(anyhow!(
                    "Ontology {} was filtered out by ontology include/exclude patterns",
                    existing.id()
                ));
            }
        }

        // Fall back to any loaded id and attach error context when nothing resolved.
        ids.into_iter().next().ok_or_else(|| {
            let mut base = format!("Failed to add ontology for location {}", location);
            if !errors.is_empty() {
                base.push_str(": ");
                base.push_str(&errors.join("; "));
            }
            anyhow!(base)
        })
    }

    fn try_reuse_cached_bytes(
        &self,
        location: &OntologyLocation,
        bytes: &[u8],
        refresh: RefreshStrategy,
    ) -> Result<Option<GraphIdentifier>> {
        // Mirror try_reuse_cached for in-memory sources by comparing content hashes.
        if !self.config.use_cached_ontologies.is_enabled() || refresh.is_force() {
            return Ok(None);
        }
        let existing = match self.env.get_ontology_by_location(location) {
            Some(ontology) => ontology,
            None => return Ok(None),
        };
        let Some(stored_hash) = existing.content_hash() else {
            return Ok(None);
        };
        let current_hash = blake3::hash(bytes).to_hex().to_string();
        if current_hash == stored_hash {
            return Ok(Some(existing.id().clone()));
        }
        Ok(None)
    }

    fn try_reuse_cached(
        &self,
        location: &OntologyLocation,
        refresh: RefreshStrategy,
    ) -> Result<Option<GraphIdentifier>> {
        // Cache reuse is only allowed when caching is enabled and refresh is not forced.
        if !self.config.use_cached_ontologies.is_enabled() {
            return Ok(None);
        }
        let existing = match self.env.get_ontology_by_location(location) {
            Some(ontology) => ontology,
            None => return Ok(None),
        };

        let existing_id = existing.id().clone();

        if refresh.is_force() {
            return Ok(None);
        }

        if let OntologyLocation::File(path) = location {
            // File-backed ontologies use content hash when available for precision.
            // Prefer content hash for accuracy
            if let Some(stored_hash) = existing.content_hash() {
                match hash_file(path) {
                    Ok(current_hash) => {
                        if current_hash == stored_hash {
                            return Ok(Some(existing_id));
                        }
                        // Hashes differ, so file is modified. Do not reuse.
                        return Ok(None);
                    }
                    Err(err) => {
                        warn!(
                            "Failed to hash file {} for cache check, falling back to mtime: {}",
                            path.display(),
                            err
                        );
                    }
                }
            }

            // Hash not available or failed; compare mtimes as a best-effort fallback.
            // Fallback to mtime comparison for legacy records without a hash
            let last_updated = match existing.last_updated {
                Some(ts) => ts,
                None => return Ok(None), // Cannot determine freshness
            };

            match self.io.source_last_modified(existing.id()) {
                Ok(source_modified) => {
                    if source_modified <= last_updated {
                        return Ok(Some(existing_id));
                    }
                }
                Err(err) => {
                    // If mtime fails, reuse to avoid unnecessary refetching.
                    warn!(
                        "Failed to determine modification time for {} ({}); using cached version",
                        existing_id, err
                    );
                    return Ok(Some(existing_id)); // Err on safe side
                }
            }

            Ok(None) // Modified or freshness uncertain
        } else {
            // Remote ontologies are reused only within the configured TTL window.
            // For URLs, reuse the cached ontology if it has not expired based on TTL.
            let ttl = chrono::Duration::from_std(std::time::Duration::from_secs(
                self.config.remote_cache_ttl_secs,
            ))
            .unwrap_or(chrono::Duration::MAX);
            if let Some(last_updated) = existing.last_updated {
                let age = Utc::now() - last_updated;
                if age <= ttl {
                    return Ok(Some(existing_id));
                }
                info!(
                    "Cached remote ontology {} expired after {:?}; refetching",
                    existing_id, age
                );
            }
            Ok(None)
        }
    }

    /// Loads or refreshes graphs discovered in the configured search directories.
    ///
    /// When `all` is `false`, only new or modified ontology sources are reparsed. When `all`
    /// is `true`, every known ontology location is reprocessed regardless of timestamps,
    /// allowing callers to force a fresh ingest of all content.
    ///
    /// The workflow removes ontologies whose sources disappeared, detects additions and
    /// updates by comparing on-disk content with the stored copy, ingests changed files, and
    /// finally refreshes the dependency graph for the affected ontologies.
    pub fn update_all(&mut self, all: bool) -> Result<Vec<GraphIdentifier>> {
        // Batch updates so dependency graph and store writes remain consistent.
        self.with_io_batch(move |env| env.update_all_inner(all))
    }

    fn update_all_inner(&mut self, all: bool) -> Result<Vec<GraphIdentifier>> {
        // Clear failure tracking so new refresh errors are reported accurately.
        self.failed_resolutions.clear();
        // Drop ontologies whose source disappeared before re-ingesting.
        self.remove_missing_ontologies()?;
        let ontology_filters = self.ontology_filters()?;
        // Prune any already-present ontologies that no longer satisfy filters
        self.prune_disallowed_ontologies(&ontology_filters, true)?;

        // Discover candidate locations (all vs only changed/new).
        let updated_files = self.collect_updated_files(all)?;
        let mut progress = ProgressReporter::new();
        progress.announce_discovered(updated_files.len());
        let seeds: Vec<PendingImport> = updated_files
            .into_iter()
            .map(|loc| PendingImport::from_location(loc, Overwrite::Allow, self.config.strict, 0))
            .collect();
        // Force refresh when requested, otherwise reuse cached where possible.
        let refresh = if all {
            RefreshStrategy::Force
        } else {
            RefreshStrategy::UseCache
        };
        let (ontologies, reused_ids, _errors) =
            self.process_import_queue(seeds, refresh, true, None, &mut progress)?;

        // Register only ontologies allowed by filters; collect reused ids too.
        let filtered_onts: Vec<Ontology> = ontologies
            .into_iter()
            .filter(|o| ontology_filters.allow(o.id()))
            .collect();
        let mut ids = self.register_ontologies(filtered_onts, true, &ontology_filters)?;
        ids.extend(
            reused_ids
                .into_iter()
                .filter(|id| ontology_filters.allow(id)),
        );
        Ok(ids)
    }

    /// Returns a list of all ontologies from the environment which have been updated.
    fn get_updated_from_environment(&self) -> Vec<GraphIdentifier> {
        self.env
            .ontologies()
            .iter()
            .filter(|(_, ontology)| {
                let location = match ontology.location() {
                    Some(loc) => loc,
                    None => {
                        // Cannot check ontologies without a location
                        return false;
                    }
                };

                let last_updated = ontology
                    .last_updated
                    .unwrap_or(Utc.timestamp_opt(0, 0).unwrap());

                match location {
                    OntologyLocation::File(path) => {
                        // Prefer a fast content hash comparison to avoid mtime granularity issues.
                        let current_hash = match hash_file(path) {
                            Ok(h) => h,
                            Err(e) => {
                                warn!(
                                    "Could not hash file for update check {}: {}",
                                    path.display(),
                                    e
                                );
                                return true; // assume updated if we cannot hash
                            }
                        };

                        if let Some(stored_hash) = ontology.content_hash() {
                            if stored_hash == current_hash {
                                return false;
                            }
                            return true;
                        }

                        // Fallback to mtime when legacy records lack a stored hash.
                        let source_modified = self
                            .io
                            .source_last_modified(ontology.id())
                            .unwrap_or(Utc::now());
                        source_modified > last_updated
                    }
                    _ => {
                        // For remote ontologies, use TTL-based staleness check
                        // instead of HEAD requests (which fall back to Utc::now()
                        // when the server has no Last-Modified header, causing
                        // unnecessary refetches every update).
                        let ttl =
                            chrono::Duration::seconds(self.config.remote_cache_ttl_secs as i64);
                        last_updated + ttl < Utc::now()
                    }
                }
            })
            .map(|(graphid, _)| graphid.clone())
            .collect()
    }

    fn remove_missing_ontologies(&mut self) -> Result<()> {
        for graphid in self.missing_ontologies() {
            self.io.remove(&graphid)?;
            self.env.remove_ontology(&graphid)?;
        }
        Ok(())
    }

    fn collect_updated_files(&mut self, all: bool) -> Result<Vec<OntologyLocation>> {
        if all {
            let mut set: HashSet<OntologyLocation> = self
                .env
                .ontologies()
                .values()
                .filter_map(|o| o.location().cloned())
                .collect();
            for loc in self.find_files()? {
                set.insert(loc);
            }
            Ok(set.into_iter().collect())
        } else {
            self.get_updated_locations()
        }
    }

    fn process_import_queue(
        &mut self,
        seeds: Vec<PendingImport>,
        refresh: RefreshStrategy,
        include_imports: bool,
        max_import_depth: Option<usize>,
        progress: &mut ProgressReporter,
    ) -> Result<(Vec<Ontology>, Vec<GraphIdentifier>, Vec<String>)> {
        // Use a BFS-style queue to load ontologies and (optionally) their imports.
        let mut queue: VecDeque<PendingImport> = seeds.into_iter().collect();
        // Track locations to prevent cycles and duplicate fetches.
        let mut seen: HashSet<OntologyLocation> = HashSet::new();
        let mut fetched: Vec<Ontology> = Vec::new();
        // Preserve insertion order of touched ids for stable outputs.
        let mut touched_ids: Vec<GraphIdentifier> = Vec::new();
        let mut touched_set: HashSet<GraphIdentifier> = HashSet::new();
        let mut errors: Vec<String> = Vec::new();
        progress.add_discovered(queue.len());

        let mut record_id = |id: &GraphIdentifier| {
            if touched_set.insert(id.clone()) {
                touched_ids.push(id.clone());
            }
        };

        while let Some(job) = queue.pop_front() {
            let (job_location_ref, job_required, job_depth) = job.meta();
            let job_location = job_location_ref.clone();
            if !seen.insert(job_location.clone()) {
                continue;
            }
            progress.loading(&job_location, queue.len() + 1);
            match self.fetch_location(job, refresh) {
                Ok(FetchOutcome::Loaded(ontology)) => {
                    let ontology = *ontology;
                    let imports = ontology.imports.clone();
                    let id = ontology.id().clone();
                    progress.tick_loaded();
                    if include_imports {
                        let import_count = imports.len();
                        if import_count > 0 {
                            progress.expanding(id.to_uri_string(), import_count);
                            progress.add_discovered(import_count);
                        }
                        self.enqueue_imports_for_job(
                            imports,
                            &mut queue,
                            job_depth,
                            max_import_depth,
                        )?;
                    }
                    fetched.push(ontology);
                    record_id(&id);
                }
                Ok(FetchOutcome::Reused(id)) => {
                    // Reused ontologies still contribute to the dependency graph.
                    progress.tick_reused();
                    record_id(&id);
                    if include_imports {
                        if let Ok(existing) = self.get_ontology(&id) {
                            let imports = existing.imports;
                            let import_count = imports.len();
                            if import_count > 0 {
                                progress.expanding(id.to_uri_string(), import_count);
                                progress.add_discovered(import_count);
                            }
                            self.enqueue_imports_for_job(
                                imports,
                                &mut queue,
                                job_depth,
                                max_import_depth,
                            )?;
                        }
                    }
                }
                Err(err) => {
                    let err_str = err.to_string();
                    let enriched = format!("Failed to load ontology {}: {}", job_location, err_str);
                    if job_required {
                        return Err(anyhow!(enriched));
                    }
                    // Non-strict mode records errors but continues processing.
                    warn!("{}", enriched);
                    errors.push(enriched);
                    if let OntologyLocation::Url(url) = &job_location {
                        if let Ok(node) = NamedNode::new(url.clone()) {
                            self.failed_resolutions.insert(node);
                        }
                    }
                }
            }
            progress.tick_processed();
        }

        Ok((fetched, touched_ids, errors))
    }

    fn enqueue_imports_for_job(
        &mut self,
        imports: Vec<NamedNode>,
        queue: &mut VecDeque<PendingImport>,
        job_depth: usize,
        max_import_depth: Option<usize>,
    ) -> Result<()> {
        let should_traverse = max_import_depth
            .map(|max_depth| job_depth < max_depth)
            .unwrap_or(true);
        if !should_traverse {
            return Ok(());
        }

        for import in imports {
            self.queue_import_location(&import, queue, self.config.strict, job_depth + 1)?;
        }
        Ok(())
    }

    fn queue_import_location(
        &mut self,
        import: &NamedNode,
        queue: &mut VecDeque<PendingImport>,
        strict: bool,
        depth: usize,
    ) -> Result<()> {
        let iri = import.as_str();
        // Only queue imports we can actually retrieve (http(s) or file).
        let is_fetchable =
            iri.starts_with("http://") || iri.starts_with("https://") || iri.starts_with("file://");
        if !is_fetchable {
            return Ok(());
        }

        // If the import is already known, reuse its resolved location.
        if let Some(existing) = self.env.get_ontology_by_name(import.into()) {
            if let Some(loc) = existing.location() {
                queue.push_back(PendingImport::from_location(
                    loc.clone(),
                    Overwrite::Preserve,
                    strict,
                    depth,
                ));
                return Ok(());
            }
        }

        // Otherwise, treat the IRI as a location and enqueue it for retrieval.
        match OntologyLocation::from_str(iri) {
            Ok(loc) => queue.push_back(PendingImport::from_location(
                loc,
                Overwrite::Preserve,
                strict,
                depth,
            )),
            Err(err) => {
                self.failed_resolutions.insert(import.clone());
                if strict {
                    return Err(err);
                }
                warn!("Failed to resolve location for import {}: {}", import, err);
            }
        }
        Ok(())
    }

    /// Returns a list of all files in the environment which have been updated (added or changed)
    /// Does not return files that have been removed
    pub fn get_updated_locations(&self) -> Result<Vec<OntologyLocation>> {
        // Combine new files on disk with modified ontologies already tracked.
        // make a cache of all files in the ontologies property
        let mut existing_files: HashSet<OntologyLocation> = HashSet::new();
        for ontology in self.env.ontologies().values() {
            if let Some(location) = ontology.location() {
                if let OntologyLocation::File(_) = location {
                    existing_files.insert(location.clone());
                }
            }
        }
        // traverse the search directories and find all files which are not in the cache
        let new_files: HashSet<OntologyLocation> = self
            .find_files()?
            .into_iter()
            .filter(|file| !existing_files.contains(file))
            .collect();

        // get the updated ontologies from the environment
        let updated_ids = self.get_updated_from_environment();
        if !updated_ids.is_empty() {
            info!("Updating ontologies: {updated_ids:?}");
        }
        let mut updated_files: HashSet<OntologyLocation> = updated_ids
            .iter()
            .filter_map(|id| {
                self.env
                    .ontologies()
                    .get(id)
                    .and_then(|ont| ont.location().cloned())
            })
            .collect::<HashSet<OntologyLocation>>();

        // compute the union of new_files and updated_files
        updated_files.extend(new_files);
        info!(
            "Found {} new or updated files in the search directories",
            updated_files.len()
        );
        Ok(updated_files.into_iter().collect())
    }

    /// Lists all ontologies in the environment which are no longer
    /// present in the search directories.
    fn missing_ontologies(&self) -> Vec<GraphIdentifier> {
        self.env
            .ontologies()
            .iter()
            .filter(|(_, ontology)| !ontology.exists())
            .map(|(graphid, _)| graphid.clone())
            .collect()
    }

    /// Returns a list of all imports that could not be resolved.
    pub fn missing_imports(&self) -> Vec<NamedNode> {
        // Report imports that are not resolvable within the current environment.
        let mut missing = HashSet::new();
        for ontology in self.env.ontologies().values() {
            for import in &ontology.imports {
                if self.env.get_ontology_by_name(import.as_ref()).is_none() {
                    missing.insert(import.clone());
                }
            }
        }
        missing.into_iter().collect()
    }

    /// Returns all imports that could not be resolved within the transitive closure
    /// of the given ontology.  Walks the full import graph starting from `id`,
    /// visiting every reachable (i.e. resolvable) ontology and collecting any
    /// declared `owl:imports` that cannot be found in the environment.
    pub fn missing_imports_in_closure(&self, id: &GraphIdentifier) -> Result<Vec<NamedNode>> {
        let mut missing: HashSet<NamedNode> = HashSet::new();
        let mut visited: HashSet<GraphIdentifier> = HashSet::new();
        let mut stack: VecDeque<GraphIdentifier> = VecDeque::new();

        stack.push_back(id.clone());
        while let Some(graph) = stack.pop_front() {
            if !visited.insert(graph.clone()) {
                continue;
            }
            let ontology = self
                .ontologies()
                .get(&graph)
                .ok_or_else(|| anyhow!("Ontology {} not found", graph.to_uri_string()))?;
            for import in &ontology.imports {
                match self.env.get_ontology_by_name(import.into()) {
                    Some(imp) => {
                        let imp_id = imp.id().clone();
                        if !visited.contains(&imp_id) {
                            stack.push_back(imp_id);
                        }
                    }
                    None => {
                        missing.insert(import.clone());
                    }
                }
            }
        }
        Ok(missing.into_iter().collect())
    }

    /// Lists all ontologies in the search directories which match
    /// the include/exclude glob patterns
    pub fn find_files(&self) -> Result<Vec<OntologyLocation>> {
        // Walk configured locations using include/exclude globs.
        if self.config.locations.is_empty() {
            return Ok(Vec::new());
        }
        let (include_set, exclude_set) = self.config.build_globsets()?;
        let includes_empty = self.config.includes_is_empty();

        let matches = |path: &Path| {
            let rel = path
                .strip_prefix(&self.config.root)
                .unwrap_or(path)
                .to_path_buf();

            if exclude_set.is_match(&rel) {
                return false;
            }
            if includes_empty {
                return true;
            }
            include_set.is_match(&rel)
        };
        let mut files = HashSet::new();
        for location in &self.config.locations {
            let resolved = crate::ontology::canonicalize_file_path(&self.resolve_path(location));
            // if location does not exist, skip it
            if !resolved.exists() {
                warn!("Location does not exist: {resolved:?}");
                continue;
            }
            // if location is a file, add it to the list
            if resolved.is_file() && matches(&resolved) {
                if let Err(err) = std::fs::File::open(&resolved) {
                    if self.config.strict {
                        return Err(err.into());
                    }
                    warn!("Skipping {:?} due to access error: {}", resolved, err);
                } else {
                    files.insert(OntologyLocation::File(
                        crate::ontology::canonicalize_file_path(&resolved),
                    ));
                }
                continue;
            }
            for entry in walkdir::WalkDir::new(&resolved) {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(err) => {
                        if self.config.strict {
                            return Err(err.into());
                        }
                        let path = err
                            .path()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| resolved.display().to_string());
                        warn!("Skipping {path} due to filesystem error: {err}");
                        continue;
                    }
                };
                if entry.file_type().is_file() && matches(entry.path()) {
                    // Skip unreadable files when not strict
                    if let Err(err) = std::fs::File::open(entry.path()) {
                        if self.config.strict {
                            return Err(err.into());
                        }
                        warn!(
                            "Skipping {:?} due to access error while opening: {}",
                            entry.path(),
                            err
                        );
                        continue;
                    }
                    files.insert(OntologyLocation::File(
                        crate::ontology::canonicalize_file_path(entry.path()),
                    ));
                }
            }
        }
        Ok(files.into_iter().collect())
    }

    /// Runs `f` against `self` inside a state snapshot. On `Ok`, the snapshot
    /// is discarded (commit). On `Err`, the snapshotted state is restored
    /// before the error propagates (rollback).
    ///
    /// NOTE: only covers the in-memory fields captured by [`EnvTransaction`].
    /// `self.io` writes are *not* rolled back; callers that mutate the IO
    /// store inside the closure must accept that orphan graphs may remain on
    /// rollback. Today this is acceptable: subsequent runs are idempotent and
    /// the orphans are pruned at next refresh.
    pub(crate) fn with_env_transaction<F, R>(&mut self, f: F) -> Result<R>
    where
        F: FnOnce(&mut OntoEnv) -> Result<R>,
    {
        let snapshot = EnvTransaction::snapshot(self);
        match f(self) {
            Ok(r) => Ok(r),
            Err(e) => {
                snapshot.restore(self);
                Err(e)
            }
        }
    }

    fn add_ids_to_dependency_graph(&mut self, ids: Vec<GraphIdentifier>) -> Result<()> {
        // Wrap the multi-step env + dep-graph rebuild in a transaction so a
        // mid-traversal error (typically `self.io.add(...)` failing in strict
        // mode after earlier successful imports) doesn't leave `self.env`,
        // `self.dependency_graph`, `self.dependency_graph_index`, and
        // `self.failed_resolutions` desynced from each other.
        //
        // NOTE: rdf5d/oxigraph store writes inside `self.io.add` are *not*
        // covered by this rollback. If we roll back env after a successful
        // io.add, the named graph remains in the store as an orphan. This is
        // a known limitation; existing refresh/prune paths reconcile.
        self.with_env_transaction(|s| s.add_ids_to_dependency_graph_inner(ids))
    }

    fn add_ids_to_dependency_graph_inner(&mut self, ids: Vec<GraphIdentifier>) -> Result<()> {
        // Walk the imports closure to ensure all reachable ontologies are loaded.
        // traverse the owl:imports closure and build the dependency graph
        let mut stack: VecDeque<GraphIdentifier> = ids.into();
        let mut seen: HashSet<GraphIdentifier> = HashSet::new();

        while let Some(graphid) = stack.pop_front() {
            debug!("Building dependency graph for: {graphid:?}");
            if seen.contains(&graphid) {
                continue;
            }
            seen.insert(graphid.clone());
            // get the ontology metadata record for this graph. If we don't have
            // it and we're in strict mode, return an error. Otherwise just skip it.
            // Use the direct id lookup; we want THIS exact graph, not whatever the
            // configured ResolutionPolicy would map this name to.
            let ontology = match self.env.get_ontology_by_id(&graphid) {
                Some(ontology) => ontology,
                None => {
                    let msg = format!("Could not find ontology: {graphid:?}");
                    if self.config.strict {
                        error!("{msg}");
                        return Err(anyhow::anyhow!(msg));
                    } else {
                        warn!("{msg}");
                        continue;
                    }
                }
            };
            let imports = &ontology.imports.clone();
            for import in imports {
                if self.failed_resolutions.contains(import) {
                    continue;
                }

                // Check if we already have an ontology with this name in the environment
                if let Some(imp) = self.env.get_ontology_by_name(import.into()) {
                    if !seen.contains(imp.id()) && !stack.contains(imp.id()) {
                        // Defer traversal so we build a complete closure.
                        stack.push_back(imp.id().clone());
                    }
                    continue;
                }

                // If not, we need to locate and add it.
                // Treat the import IRI as a location.
                let location = match OntologyLocation::from_str(import.as_str()) {
                    Ok(loc) => loc,
                    Err(e) => {
                        self.failed_resolutions.insert(import.clone());
                        if self.config.strict {
                            return Err(e);
                        }
                        warn!(
                            "Failed to resolve location for import {}: {}",
                            import.as_str(),
                            e
                        );
                        continue;
                    }
                };

                match self.io.add(location, Overwrite::Preserve) {
                    Ok(new_ont) => {
                        let id = new_ont.id().clone();
                        // Register newly discovered imports so edges can be created later.
                        self.env.add_ontology(new_ont)?;
                        stack.push_back(id);
                    }
                    Err(e) => {
                        self.failed_resolutions.insert(import.clone());
                        if self.config.strict {
                            return Err(e);
                        }
                        warn!("Failed to read ontology file {}: {}", import.as_str(), e);
                        continue;
                    }
                }
            }
        }
        // Rebuild the dependency graph from the current environment snapshot.
        let mut indexes: HashMap<GraphIdentifier, NodeIndex> = HashMap::new();
        let mut graph: DiGraph<GraphIdentifier, (), petgraph::Directed> = DiGraph::new();
        // add all ontologies in self.ontologies to the graph
        for ontology in self.env.ontologies().keys() {
            let index = graph.add_node(ontology.clone());
            indexes.insert(ontology.clone(), index);
        }
        // traverse the ontologies and add edges to the graph
        for ontology in self.env.ontologies().keys() {
            let index = indexes.get(ontology).ok_or_else(|| {
                anyhow!(
                    "Programming error: ontology id {:?} not in index map",
                    ontology
                )
            })?;
            let ont = match self.env.ontologies().get(ontology) {
                Some(ont) => ont,
                None => {
                    error!("Ontology not found: {ontology:?}");
                    continue;
                }
            };
            for import in &ont.imports {
                let graph_id = match self.env.get_ontology_by_name(import.into()) {
                    Some(imp) => imp.id(),
                    None => {
                        if self.config.strict {
                            return Err(anyhow::anyhow!("Import not found: {}", import));
                        }
                        warn!("Import not found: {import}");
                        continue;
                    }
                };
                let import_index = indexes.get(graph_id).ok_or_else(|| {
                    anyhow!(
                        "Programming error: ontology id {:?} not in index map",
                        graph_id
                    )
                })?;
                // Edge direction is importer -> import to match dependency semantics.
                graph.add_edge(*index, *import_index, ());
            }
        }
        self.dependency_graph = graph;
        self.dependency_graph_index = indexes;
        Ok(())
    }

    fn rebuild_dependency_graph(&mut self) -> Result<()> {
        let ids: Vec<GraphIdentifier> = self.env.ontologies().keys().cloned().collect();
        self.dependency_graph = DiGraph::new();
        self.dependency_graph_index.clear();
        if ids.is_empty() {
            return Ok(());
        }
        self.add_ids_to_dependency_graph(ids)
    }

    /// Rebuild derived dependency indexes strictly from catalog metadata.
    /// Missing imports remain unresolved; this method never reads or fetches
    /// ontology graph content.
    fn rebuild_dependency_graph_from_metadata(&mut self) {
        let mut graph = DiGraph::new();
        let mut indexes = HashMap::new();
        for id in self.env.ontologies().keys() {
            let index = graph.add_node(id.clone());
            indexes.insert(id.clone(), index);
        }
        for ontology in self.env.ontologies().values() {
            let Some(source) = indexes.get(ontology.id()).copied() else {
                continue;
            };
            for import in &ontology.imports {
                let Some(imported) = self.env.get_ontology_by_name(import.as_ref()) else {
                    continue;
                };
                if let Some(target) = indexes.get(imported.id()).copied() {
                    graph.add_edge(source, target, ());
                }
            }
        }
        self.dependency_graph = graph;
        self.dependency_graph_index = indexes;
    }

    /// Returns a list of issues with the environment
    pub fn doctor(&self) -> Result<Vec<OntologyProblem>> {
        // Run the default set of environment checks.
        let mut doctor = Doctor::new();
        doctor.add_check(Box::new(DuplicateOntology {}));
        doctor.add_check(Box::new(OntologyDeclaration {}));
        doctor.add_check(Box::new(ConflictingPrefixes {}));

        doctor.run(self)
    }

    /// Returns the dependency closure for the provided graph identifier.
    ///
    /// The returned vector contains `GraphIdentifier`s, with the requested identifier inserted
    /// at the front followed by its resolved imports. If `recursion_depth` is non-negative,
    /// traversal stops once that depth is reached. In strict mode an unresolved import results
    /// in an error; otherwise the missing import is logged and skipped.
    pub fn get_closure(
        &self,
        id: &GraphIdentifier,
        recursion_depth: i32,
    ) -> Result<Vec<GraphIdentifier>> {
        // Walk the pre-built dependency graph via BFS. This avoids per-step
        // import name resolution (which would otherwise fall through the alias
        // map into a linear scan of every ontology in the environment).
        if !self.ontologies().contains_key(id) {
            return Err(anyhow!("Ontology {} not found", id.to_uri_string()));
        }

        let Some(&root_idx) = self.dependency_graph_index.get(id) else {
            // No dep graph entry. With env mutations now transactional in
            // `add_ids_to_dependency_graph`, the only way to reach this branch
            // is the intentional `add(..., update_dependencies=false)` public
            // API path. Fall back to the legacy name-resolution traversal so
            // the closure is still correct, at the cost of per-step linear
            // lookups.
            return self.get_closure_via_name_resolution(id, recursion_depth);
        };

        let mut result: Vec<GraphIdentifier> = Vec::new();
        result.push(id.clone());
        let mut visited: HashSet<NodeIndex> = HashSet::with_capacity(8);
        visited.insert(root_idx);
        let mut queue: VecDeque<(NodeIndex, i32)> = VecDeque::new();
        queue.push_back((root_idx, 0));

        while let Some((idx, depth)) = queue.pop_front() {
            if recursion_depth >= 0 && depth >= recursion_depth {
                continue;
            }
            for edge in self
                .dependency_graph
                .edges_directed(idx, petgraph::Direction::Outgoing)
            {
                let target = edge.target();
                if visited.insert(target) {
                    let dep_id = &self.dependency_graph[target];
                    result.push(dep_id.clone());
                    queue.push_back((target, depth + 1));
                }
            }
        }

        // In strict mode, surface unresolved imports as an error to match prior
        // semantics. Use `failed_resolutions` rather than re-resolving by
        // name: the former records exactly the imports we couldn't resolve
        // during `add_ids_to_dependency_graph` (which uses id-level lookups),
        // so it stays consistent with the dependency-graph edges. Re-resolving
        // via `get_ontology_by_name` would route through `ResolutionPolicy`
        // and could disagree with the graph the BFS just walked.
        if self.config.strict {
            for graph_id in &result {
                let ontology = self
                    .ontologies()
                    .get(graph_id)
                    .ok_or_else(|| anyhow!("Ontology {} not found", graph_id.to_uri_string()))?;
                for import in &ontology.imports {
                    if self.failed_resolutions.contains(import) {
                        return Err(anyhow!("Import not found: {}", import));
                    }
                }
            }
        }

        info!("Dependency closure for {:?}: {:?}", id, result.len());
        Ok(result)
    }

    /// Fallback closure traversal that resolves imports by name on each step
    /// instead of walking `dependency_graph`. Used by [`get_closure`] when the
    /// requested id isn't represented in `dependency_graph_index` (e.g. an
    /// ontology was added without updating the dependency graph). Slower than
    /// the indexed BFS path because it does a name lookup per import, but does
    /// not require `dependency_graph_index` to be populated.
    fn get_closure_via_name_resolution(
        &self,
        id: &GraphIdentifier,
        recursion_depth: i32,
    ) -> Result<Vec<GraphIdentifier>> {
        let mut closure: HashSet<GraphIdentifier> = HashSet::new();
        let mut order: Vec<GraphIdentifier> = Vec::new();
        let mut stack: VecDeque<(GraphIdentifier, i32)> = VecDeque::new();
        stack.push_back((id.clone(), 0));
        while let Some((graph, depth)) = stack.pop_front() {
            if !closure.insert(graph.clone()) {
                continue;
            }
            order.push(graph.clone());

            if recursion_depth >= 0 && depth >= recursion_depth {
                continue;
            }

            let ontology = self
                .ontologies()
                .get(&graph)
                .ok_or_else(|| anyhow!("Ontology {} not found", graph.to_uri_string()))?;
            for import in &ontology.imports {
                let import = match self.env.get_ontology_by_name(import.into()) {
                    Some(imp) => imp.id().clone(),
                    None => {
                        if self.config.strict {
                            return Err(anyhow!("Import not found: {}", import));
                        }
                        warn!("Import not found: {import}");
                        continue;
                    }
                };
                if !closure.contains(&import) {
                    stack.push_back((import, depth + 1));
                }
            }
        }
        info!(
            "Dependency closure for {:?} (fallback): {:?}",
            id,
            order.len()
        );
        Ok(order)
    }

    pub fn get_union_graph<'a, I>(
        &self,
        graph_ids: I,
        root: NamedNodeRef,
        rewrite_sh_prefixes: Option<bool>,
        remove_owl_imports: Option<bool>,
    ) -> Result<UnionGraph>
    where
        I: IntoIterator<Item = &'a GraphIdentifier>,
    {
        // Merge multiple graphs into a dataset with optional cleanup transforms.
        let graph_ids: Vec<GraphIdentifier> = graph_ids.into_iter().cloned().collect();

        if graph_ids.is_empty() {
            return Err(anyhow!("No graphs found"));
        }

        // One bulk call into the store. `union_graph` is always best-effort:
        // it records per-id failures in `failures` and assembles the rest.
        // Strict mode promotes any failure to an error here; non-strict mode
        // returns the partial union with `failed_imports` populated so the
        // caller knows what's missing.
        let (mut dataset, failures) = self.io.union_graph(&graph_ids);
        if self.config.strict && !failures.is_empty() {
            return Err(anyhow!(
                "union_graph: {} graph(s) failed to load: {}",
                failures.len(),
                failures
                    .iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        for f in &failures {
            warn!("Skipping graph in union: {f}");
        }
        let failed_imports = (!failures.is_empty()).then_some(failures);
        let root_ontology = NamedOrBlankNodeRef::NamedNode(root);

        // Merge namespace maps so downstream tools can re-materialize prefixes.
        // Borrow ontologies directly from the environment to avoid cloning each
        // Ontology and running the resolution policy per closure entry.
        let mut namespace_map = HashMap::new();
        for graph_id in &graph_ids {
            let ontology = self
                .env
                .ontologies()
                .get(graph_id)
                .ok_or_else(|| anyhow!("Ontology {} not found", graph_id.to_uri_string()))?;
            namespace_map.extend(
                ontology
                    .namespace_map()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
        }

        // Rewrite sh:prefixes
        // defaults to true if not specified
        if rewrite_sh_prefixes.unwrap_or(true) {
            transform::rewrite_sh_prefixes_dataset(&mut dataset, root_ontology)?;
        }
        // remove owl:imports
        if remove_owl_imports.unwrap_or(true) {
            let to_remove: Vec<NamedNodeRef> = graph_ids.iter().map(|id| id.into()).collect();
            transform::remove_owl_imports(&mut dataset, Some(&to_remove));
        }
        // Collapse ontology declarations onto the chosen root.
        transform::remove_ontology_declarations(&mut dataset, root_ontology);
        let has_root_declaration = dataset.iter().any(|q| {
            q.subject == root_ontology
                && q.predicate == TYPE
                && q.object == TermRef::NamedNode(ONTOLOGY)
        });
        if !has_root_declaration {
            dataset.insert(QuadRef::new(
                root_ontology,
                TYPE,
                ONTOLOGY,
                GraphNameRef::DefaultGraph,
            ));
        }
        Ok(UnionGraph {
            dataset,
            graph_ids,
            failed_imports,
            namespace_map,
        })
    }

    /// Resolve an explicitly enumerated set of graph IRIs and return their union.
    ///
    /// When `include_closures` is true, each enumerated graph contributes its
    /// full `owl:imports` closure. Otherwise only the enumerated graphs are
    /// included. The `root` IRI is used only for union normalization
    /// (`sh:prefixes` rewriting and ontology declaration cleanup); it does not
    /// need to be one of the enumerated graphs.
    pub fn get_explicit_union_graph(
        &self,
        graph_iris: &[NamedNode],
        root: NamedNodeRef,
        include_closures: bool,
        recursion_depth: i32,
        rewrite_sh_prefixes: Option<bool>,
        remove_owl_imports: Option<bool>,
    ) -> Result<UnionGraph> {
        if graph_iris.is_empty() {
            return Err(anyhow!("No graphs specified"));
        }

        let mut graph_ids = Vec::new();
        let mut seen = HashSet::new();
        for iri in graph_iris {
            let id = self
                .resolve(ResolveTarget::Graph(iri.clone()))
                .ok_or_else(|| anyhow!("Ontology {} not found", iri.as_str()))?;
            if include_closures {
                for closure_id in self.get_closure(&id, recursion_depth)? {
                    if seen.insert(closure_id.clone()) {
                        graph_ids.push(closure_id);
                    }
                }
            } else if seen.insert(id.clone()) {
                graph_ids.push(id);
            }
        }

        self.get_union_graph(&graph_ids, root, rewrite_sh_prefixes, remove_owl_imports)
    }

    /// Collect namespace prefixes for a single ontology.
    ///
    /// Two sources are merged (in order of increasing priority):
    ///
    /// 1. **Parser-level prefixes** — `@prefix` / `PREFIX` declarations obtained
    ///    by re-reading the ontology's source file or URL.
    /// 2. **SHACL `sh:declare` entries** — stored in [`Ontology::namespace_map`].
    ///    These take precedence when the same prefix name appears in both sources.
    ///
    /// If the source cannot be re-read (e.g. in-memory location, missing file),
    /// the error is logged and only SHACL entries are returned.
    fn collect_ontology_prefixes(&self, ontology: &Ontology) -> HashMap<String, String> {
        // Start with parser-level @prefix / PREFIX declarations from the source.
        let mut namespace_map = ontology
            .location()
            .map(|loc| {
                crate::util::read_prefixes_from_location(loc).unwrap_or_else(|e| {
                    warn!("Failed to read prefixes from {}: {}", loc, e);
                    HashMap::new()
                })
            })
            .unwrap_or_default();
        // SHACL sh:declare entries take precedence over parser-level prefixes.
        namespace_map.extend(
            ontology
                .namespace_map()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        namespace_map
    }

    /// Return namespace (prefix → IRI) mappings for an ontology.
    ///
    /// Prefixes come from two sources: parser-level `@prefix` / `PREFIX`
    /// declarations (obtained by re-reading the source file) and SHACL
    /// `sh:declare` entries stored in the ontology metadata.  When the same
    /// prefix name appears in both, the SHACL value wins.
    ///
    /// # Arguments
    ///
    /// * `id` — The graph identifier of the target ontology.
    /// * `include_closure` — When `true`, the namespace maps of all ontologies
    ///   in the transitive `owl:imports` closure are merged (later entries win
    ///   on prefix conflicts).  When `false`, only the single ontology's
    ///   prefixes are returned.
    ///
    /// # Errors
    ///
    /// Returns an error if `id` is not found in the environment or (in strict
    /// mode) if any import in the closure cannot be resolved.
    pub fn get_namespaces(
        &self,
        id: &GraphIdentifier,
        include_closure: bool,
    ) -> Result<HashMap<String, String>> {
        if include_closure {
            let closure = self.get_closure(id, -1)?;
            let mut namespace_map = HashMap::new();
            for graph_id in &closure {
                let ontology =
                    self.env.ontologies().get(graph_id).ok_or_else(|| {
                        anyhow!("Ontology {} not found", graph_id.to_uri_string())
                    })?;
                namespace_map.extend(self.collect_ontology_prefixes(ontology));
            }
            Ok(namespace_map)
        } else {
            let ontology = self
                .env
                .ontologies()
                .get(id)
                .ok_or_else(|| anyhow!("Ontology {} not found", id.to_uri_string()))?;
            Ok(self.collect_ontology_prefixes(ontology))
        }
    }

    /// Return merged namespace (prefix → IRI) mappings across **all** ontologies
    /// in the environment.
    ///
    /// This is equivalent to calling [`get_namespaces`](Self::get_namespaces)
    /// for every ontology and merging the results.  When two ontologies declare
    /// the same prefix with different IRIs, the last one encountered wins
    /// (iteration order is not guaranteed).
    pub fn get_all_namespaces(&self) -> HashMap<String, String> {
        let mut namespace_map = HashMap::new();
        for ontology in self.ontologies().values() {
            namespace_map.extend(self.collect_ontology_prefixes(ontology));
        }
        namespace_map
    }

    /// Merge an ontology and its imports closure into a single graph.
    ///
    /// - `recursion_depth` follows the semantics of [`get_closure`]; `-1` means unlimited.
    /// - SHACL prefixes are rewritten to the requested ontology and `sh:declare` entries deduplicated.
    /// - `owl:imports` statements are removed to prevent downstream refetching.
    /// - Additional `owl:Ontology` declarations are stripped, keeping only the requested ontology.
    pub fn import_graph(&self, id: &GraphIdentifier, recursion_depth: i32) -> Result<Graph> {
        // Produce a flattened graph with imports resolved and normalized.
        let root = id.name();
        let imported = self.get_ontology(id)?;
        let imported_imports = imported.imports.clone();

        // Gather closure and merge into a dataset without transforms applied yet.
        let closure = self.get_closure(id, recursion_depth)?;
        let mut union = self.get_union_graph(&closure, root, Some(false), Some(false))?;

        let root_nb = NamedOrBlankNodeRef::NamedNode(root);
        // Apply transforms with the requested root.
        transform::rewrite_sh_prefixes_dataset(&mut union.dataset, root_nb)?;
        transform::remove_owl_imports(&mut union.dataset, None);
        transform::remove_ontology_declarations(&mut union.dataset, root_nb);

        // Flatten dataset into a single graph, ignoring named graph labels.
        // No need to filter owl:imports here: `remove_owl_imports` above already
        // stripped every owl:imports triple from the dataset.
        let mut graph = Graph::new();
        for quad in union.dataset.iter() {
            graph.insert(TripleRef::new(quad.subject, quad.predicate, quad.object));
        }
        // Re-attach imports of the imported ontology and its dependencies onto the root; skip self-imports and dedup.
        let closure_names: std::collections::HashSet<NamedNodeRef> =
            closure.iter().map(|id| id.name()).collect();
        let mut seen = std::collections::HashSet::new();
        let mut add_import = |target: NamedNodeRef, dep: NamedNodeRef| {
            if target == dep {
                return;
            }
            if seen.insert(dep.to_string()) {
                graph.insert(TripleRef::new(target, IMPORTS, dep));
            }
        };
        // Preserve the ontology's declared imports that are still within the closure.
        for dep in imported_imports {
            if closure_names.contains(&dep.as_ref()) {
                add_import(root, dep.as_ref());
            }
        }
        // Add the remaining closure nodes as imports to retain full dependency context.
        for dep_id in closure.iter().skip(1) {
            add_import(root, dep_id.name());
        }
        Ok(graph)
    }

    /// Returns a read-only graph from the IO store for the given identifier.
    ///
    /// This is the primary read path and dispatches to the underlying backend
    /// (`PersistentGraphIO`, `PythonGraphIO`, etc.). The returned graph is a
    /// snapshot and should not be mutated; use [`Self::copy_graph`] when you
    /// need a mutable copy.
    pub fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        // Delegate graph retrieval to the IO backend.
        self.io.get_graph(id)
    }

    /// Returns a mutable copy of the graph for the given identifier.
    ///
    /// Unlike [`Self::get_graph`], this returns a fresh in-memory copy that
    /// can be freely mutated. Custom `GraphIO` backends can override
    /// `copy_graph` to distinguish between returning a live view (via
    /// `get_graph`) and returning a detached copy (via `copy_graph`).
    pub fn copy_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        self.io.copy_graph(id)
    }

    pub fn get_ontology(&self, id: &GraphIdentifier) -> Result<Ontology> {
        // Return a cloned ontology or a user-friendly error.
        self.env
            .get_ontology(id)
            .ok_or_else(|| anyhow!("Ontology not found"))
    }

    /// Returns a list of all ontologies that import the given ontology
    pub fn get_importers(&self, id: &NamedNode) -> Result<Vec<GraphIdentifier>> {
        // Traverse the dependency graph to find incoming edges.
        // find all nodes in the dependency_graph which have an edge to the given node
        // and return the list of nodes
        let mut importers: Vec<GraphIdentifier> = Vec::new();
        let node = self
            .env
            .get_ontology_by_name(id.into())
            .ok_or_else(|| anyhow!("Ontology not found"))?;
        let index = self
            .dependency_graph
            .node_indices()
            .find(|i| self.dependency_graph[*i] == *node.id())
            .ok_or_else(|| anyhow!("Node not found"))?;
        for edge in self
            .dependency_graph
            .edges_directed(index, petgraph::Direction::Incoming)
        {
            let importer = self.dependency_graph[edge.source()].clone();
            importers.push(importer);
        }
        Ok(importers)
    }

    /// Returns all importer paths that terminate at the given ontology.
    /// Each path is ordered from the most distant importer down to `id`.
    pub fn get_import_paths(&self, id: &NamedNode) -> Result<Vec<Vec<GraphIdentifier>>> {
        // Provide only resolved paths, erroring if the ontology is missing.
        match self.explain_import(id)? {
            ImportPaths::Present(paths) => Ok(paths),
            ImportPaths::Missing { .. } => Err(anyhow!("Ontology not found")),
        }
    }

    pub fn explain_import(&self, id: &NamedNode) -> Result<ImportPaths> {
        // Return either full import paths or partial paths for missing targets.
        if let Some(target) = self.env.get_ontology_by_name(id.into()) {
            let idx = self
                .dependency_graph
                .node_indices()
                .find(|i| self.dependency_graph[*i] == *target.id())
                .ok_or_else(|| anyhow!("Node not found"))?;
            return Ok(ImportPaths::Present(
                self.collect_import_paths_from_index(idx),
            ));
        }

        let mut importers = Vec::new();
        for ontology in self.env.ontologies().values() {
            if ontology.imports.iter().any(|imp| imp == id) {
                importers.push(ontology.id().clone());
            }
        }

        if importers.is_empty() {
            return Ok(ImportPaths::Missing {
                importers: Vec::new(),
            });
        }

        let mut paths: Vec<Vec<GraphIdentifier>> = Vec::new();
        for importer in importers {
            let maybe_idx = self
                .dependency_graph
                .node_indices()
                .find(|i| self.dependency_graph[*i] == importer);
            if let Some(idx) = maybe_idx {
                let mut importer_paths = self.collect_import_paths_from_index(idx);
                paths.append(&mut importer_paths);
            } else {
                paths.push(vec![importer.clone()]);
            }
        }

        Ok(ImportPaths::Missing { importers: paths })
    }

    fn collect_import_paths_from_index(
        &self,
        target_idx: petgraph::graph::NodeIndex,
    ) -> Vec<Vec<GraphIdentifier>> {
        // DFS over incoming edges to find all importer chains.
        let mut results: Vec<Vec<GraphIdentifier>> = Vec::new();
        let mut path: Vec<GraphIdentifier> = Vec::new();
        let mut seen: std::collections::HashSet<GraphIdentifier> = std::collections::HashSet::new();

        fn dfs(
            g: &petgraph::Graph<GraphIdentifier, (), petgraph::Directed>,
            idx: petgraph::graph::NodeIndex,
            path: &mut Vec<GraphIdentifier>,
            seen: &mut std::collections::HashSet<GraphIdentifier>,
            results: &mut Vec<Vec<GraphIdentifier>>,
        ) {
            let current = g[idx].clone();
            if !seen.insert(current.clone()) {
                // Avoid cycles in graphs with circular imports.
                return;
            }
            path.push(current.clone());

            let mut incoming = g
                .neighbors_directed(idx, petgraph::Direction::Incoming)
                .detach();

            let mut has_incoming = false;
            while let Some((_, src)) = incoming.next(g) {
                has_incoming = true;
                // Recurse toward importers (incoming edges).
                dfs(g, src, path, seen, results);
            }
            if !has_incoming {
                // Leaf reached; reverse to return path from root importer to target.
                let mut p = path.clone();
                p.reverse();
                results.push(p);
            }

            path.pop();
            seen.remove(&current);
        }

        dfs(
            &self.dependency_graph,
            target_idx,
            &mut path,
            &mut seen,
            &mut results,
        );
        results
    }

    /// Returns the GraphViz dot representation of the dependency graph
    pub fn dep_graph_to_dot(&self) -> Result<String> {
        // Render the full dependency graph to GraphViz DOT.
        self.rooted_dep_graph_to_dot(self.ontologies().keys().cloned().collect())
    }

    /// Return the GraphViz dot representation of the dependency graph
    /// rooted at the given graph
    pub fn rooted_dep_graph_to_dot(&self, roots: Vec<GraphIdentifier>) -> Result<String> {
        // Render a subgraph rooted at specific ontologies.
        let mut graph = DiGraph::new();
        let mut stack: VecDeque<GraphIdentifier> = VecDeque::new();
        let mut seen: HashSet<GraphIdentifier> = HashSet::new();
        let mut indexes: HashMap<GraphIdentifier, NodeIndex> = HashMap::new();
        let mut edges: HashSet<(NodeIndex, NodeIndex)> = HashSet::new();
        for root in roots {
            stack.push_back(root.clone());
        }
        while let Some(ontology) = stack.pop_front() {
            let index = *indexes
                .entry(ontology.clone())
                .or_insert_with(|| graph.add_node(ontology.name().into_owned()));
            let ont = self
                .ontologies()
                .get(&ontology)
                .ok_or_else(|| anyhow!("Listing ontologies: Ontology {} not found", ontology))?;
            for import in &ont.imports {
                let import = match self.env.get_ontology_by_name(import.into()) {
                    Some(imp) => imp.id().clone(),
                    None => {
                        warn!("Import not found: {import}");
                        continue;
                    }
                };
                let name: NamedNode = import.name().into_owned();
                let import_index = *indexes
                    .entry(import.clone())
                    .or_insert_with(|| graph.add_node(name));
                if !seen.contains(&import) {
                    stack.push_back(import.clone());
                }
                if !edges.contains(&(index, import_index)) {
                    graph.add_edge(index, import_index, ());
                    edges.insert((index, import_index));
                }
            }
            seen.insert(ontology);
        }
        let dot =
            petgraph::dot::Dot::with_config(&graph, &[petgraph::dot::Config::GraphContentOnly]);

        Ok(format!("digraph {{\nrankdir=LR;\n{dot:?}}}"))
    }

    /// Outputs a human-readable dump of the environment, including all ontologies
    /// and their metadata and imports
    pub fn dump(&self, contains: Option<&str>) {
        // Print a human-readable inventory for debugging and inspection.
        let mut ontologies = self.ontologies().clone();
        let mut groups: HashMap<NamedNode, Vec<Ontology>> = HashMap::new();
        for ontology in ontologies.values_mut() {
            let name = ontology.name();
            groups.entry(name).or_default().push(ontology.clone());
        }
        let mut sorted_groups: Vec<NamedNode> = groups.keys().cloned().collect();
        sorted_groups.sort();
        for name in sorted_groups {
            if let Some(contains) = contains {
                if !name.to_string().contains(contains) {
                    continue;
                }
            }
            let group = groups.get(&name).unwrap();
            println!("┌ Ontology: {name}");
            for ontology in group {
                let g = match self.io.get_graph(ontology.id()) {
                    Ok(g) => g,
                    Err(e) => {
                        warn!("Could not get graph for {}: {e}", ontology.id());
                        continue;
                    }
                };
                let loc = ontology
                    .location()
                    .map(|l| l.to_string())
                    .unwrap_or_else(|| "N/A".to_string());
                println!("├─ Location: {}", loc);
                // sorted keys
                let mut sorted_keys: Vec<NamedNode> =
                    ontology.version_properties().keys().cloned().collect();
                sorted_keys.sort();
                // print up until last key
                if !sorted_keys.is_empty() {
                    println!("│ ├─ Version properties:");
                    if sorted_keys.len() > 1 {
                        for key in sorted_keys.iter().take(sorted_keys.len() - 1) {
                            println!(
                                "│ ├─ {}: {}",
                                key,
                                ontology.version_properties().get(key).unwrap()
                            );
                        }
                    }
                    // print last key
                    println!(
                        "│ └─ {}: {}",
                        sorted_keys.last().unwrap(),
                        ontology
                            .version_properties()
                            .get(sorted_keys.last().unwrap())
                            .unwrap()
                    );
                }
                println!("│ ├─ Last updated: {}", ontology.last_updated.unwrap());
                if !ontology.imports.is_empty() {
                    println!("│ ├─ Triples: {}", g.len());
                    println!("│ ├─ Imports:");
                    let mut sorted_imports: Vec<NamedNode> = ontology.imports.clone();
                    sorted_imports.sort();
                    // print up until last import
                    for import in sorted_imports.iter().take(sorted_imports.len() - 1) {
                        println!("│ │ ├─ {import}");
                    }
                    // print last import
                    println!("│ │ └─ {}", sorted_imports.last().unwrap());
                } else {
                    println!("│ └─ Triples: {}", g.len());
                }
            }
            println!("└────────────────────────────────────────────────────────────────────────");
        }
    }

    // Config accessors
    pub fn is_offline(&self) -> bool {
        // Expose current offline mode for callers that gate network operations.
        self.config.offline
    }

    pub fn set_offline(&mut self, offline: bool) {
        // Update offline mode; caller is responsible for reloading if needed.
        self.config.offline = offline;
    }

    pub fn is_strict(&self) -> bool {
        // Expose strict mode for conditional error handling.
        self.config.strict
    }

    pub fn set_strict(&mut self, strict: bool) {
        // Update strict mode for future operations.
        self.config.strict = strict;
    }

    pub fn requires_ontology_names(&self) -> bool {
        // Expose whether ontology name declarations are required.
        self.config.require_ontology_names
    }

    pub fn set_require_ontology_names(&mut self, require: bool) {
        // Toggle name requirement to influence future imports/updates.
        self.config.require_ontology_names = require;
    }

    /// Set the TTL (in seconds) for caching remote ontologies.
    ///
    /// Remote ontologies fetched via HTTP are cached locally. When the TTL
    /// expires the next `update` or `add` triggers a re-fetch. The default
    /// is 3600 seconds (1 hour).
    pub fn set_remote_cache_ttl_secs(&mut self, ttl_secs: u64) {
        self.config.remote_cache_ttl_secs = ttl_secs;
    }

    /// Set the cache mode for ontology loading.
    ///
    /// Controls whether `update` re-loads ontologies from their original
    /// source (file/URL) or uses cached copies. See [`CacheMode`] for
    /// available modes.
    pub fn set_use_cached_ontologies(&mut self, mode: crate::options::CacheMode) {
        self.config.use_cached_ontologies = mode;
    }

    /// Apply scalar mode flags (offline, strict, etc.) from `source` to this
    /// env's persisted config, leaving list fields (locations, includes, …)
    /// untouched. Returns `true` if any field actually changed.
    fn merge_scalar_flags(&mut self, source: &Config) -> bool {
        let c = &mut self.config;
        let mut changed = false;
        if c.offline != source.offline {
            c.offline = source.offline;
            changed = true;
        }
        if c.strict != source.strict {
            c.strict = source.strict;
            changed = true;
        }
        if c.require_ontology_names != source.require_ontology_names {
            c.require_ontology_names = source.require_ontology_names;
            changed = true;
        }
        if c.remote_cache_ttl_secs != source.remote_cache_ttl_secs {
            c.remote_cache_ttl_secs = source.remote_cache_ttl_secs;
            changed = true;
        }
        if c.use_cached_ontologies != source.use_cached_ontologies {
            c.use_cached_ontologies = source.use_cached_ontologies;
            changed = true;
        }
        if c.resolution_policy != source.resolution_policy {
            c.resolution_policy = source.resolution_policy.clone();
            changed = true;
        }
        changed
    }

    pub fn resolution_policy(&self) -> &str {
        // Expose the current policy name for display and persistence.
        &self.config.resolution_policy
    }

    pub fn set_resolution_policy(&mut self, policy: String) {
        // Update policy name; actual policy is resolved when needed.
        self.config.resolution_policy = policy;
    }
}

fn hash_file(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::CacheMode;
    use oxigraph::io::RdfFormat;
    use tempfile::tempdir;

    #[test]
    fn open_or_init_initializes_when_missing() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("env");
        std::fs::create_dir_all(&root).unwrap();

        let config = Config::builder()
            .root(root.clone())
            .offline(true)
            .temporary(false)
            .locations(vec![])
            .build()
            .unwrap();

        {
            let env = OntoEnv::open_or_init(config.clone(), false).unwrap();
            assert!(root.join(".ontoenv").is_dir());
            drop(env);
        }

        {
            let env = OntoEnv::open_or_init(config, false).unwrap();
            assert!(root.join(".ontoenv").is_dir());
            drop(env);
        }
    }

    #[test]
    fn pending_marker_blocks_startup_with_recovery_error() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("env");
        std::fs::create_dir_all(&root).unwrap();
        let config = Config::builder()
            .root(root.clone())
            .offline(true)
            .temporary(false)
            .locations(vec![])
            .use_cached_ontologies(CacheMode::Enabled)
            .build()
            .unwrap();
        let environment = OntoEnv::init(config, false).unwrap();
        drop(environment);

        std::fs::write(
            root.join(".ontoenv").join(catalog::PENDING_FILE),
            r#"{"mutation_id":"test","graphs":["https://example.org/o"]}"#,
        )
        .unwrap();
        let error = OntoEnv::load_from_directory(root, false).unwrap_err();
        assert!(error.downcast_ref::<CatalogRecoveryError>().is_some());
        assert!(error.to_string().contains("https://example.org/o"));
    }

    #[test]
    fn legacy_environment_migrates_without_removing_rollback_files() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("env");
        std::fs::create_dir_all(&root).unwrap();
        let config = Config::builder()
            .root(root.clone())
            .offline(true)
            .temporary(false)
            .locations(vec![])
            .use_cached_ontologies(CacheMode::Enabled)
            .build()
            .unwrap();
        let environment = OntoEnv::init(config, false).unwrap();
        let legacy_path = root.join(".ontoenv").join("environment.json");
        write_json_file(&legacy_path, &environment.env).unwrap();
        drop(environment);
        std::fs::remove_file(root.join(".ontoenv").join(catalog::CATALOG_FILE)).unwrap();

        let migrated = OntoEnv::load_from_directory(root.clone(), false).unwrap();
        assert!(migrated.ontologies().is_empty());
        assert!(root.join(".ontoenv").join(catalog::CATALOG_FILE).exists());
        assert!(legacy_path.exists());
    }

    #[test]
    fn legacy_environment_rejects_backend_id_mismatch() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("env");
        std::fs::create_dir_all(&root).unwrap();
        let ontology_path = root.join("ontology.ttl");
        std::fs::write(
            &ontology_path,
            "<https://example.org/legacy> a <http://www.w3.org/2002/07/owl#Ontology> .",
        )
        .unwrap();
        let config = Config::builder()
            .root(root.clone())
            .offline(true)
            .temporary(false)
            .locations(vec![root.clone()])
            .build()
            .unwrap();
        let environment = OntoEnv::init(config, false).unwrap();
        let id = environment.ontologies().keys().next().unwrap().clone();
        let legacy_path = root.join(".ontoenv").join("environment.json");
        write_json_file(&legacy_path, &environment.env).unwrap();
        drop(environment);
        std::fs::remove_file(root.join(".ontoenv").join(catalog::CATALOG_FILE)).unwrap();

        let mut backend =
            crate::io::PersistentGraphIO::new(root.join(".ontoenv"), true, false).unwrap();
        backend.remove(&id).unwrap();
        backend.flush().unwrap();
        drop(backend);

        let error = OntoEnv::load_from_directory(root, false).unwrap_err();
        assert!(error
            .to_string()
            .contains("legacy metadata does not match backend graph IDs"));
        assert!(legacy_path.exists());
    }

    #[test]
    fn remote_cache_ttl_expires() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let config = Config::builder()
            .root(root.clone())
            .offline(true)
            .temporary(true)
            .locations(vec![])
            .use_cached_ontologies(CacheMode::Enabled)
            .remote_cache_ttl_secs(1)
            .build()
            .unwrap();
        let mut env = OntoEnv::init(config, true).unwrap();
        env.update_all(false).unwrap();

        let location = OntologyLocation::Url("http://example.com/ttl-cache".to_string());
        let ttl_bytes = b"@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.com/ttl-cache> a owl:Ontology .";

        // Seed the ontology directly into the store/environment.
        let ontology = env
            .io
            .add_from_bytes(
                location.clone(),
                ttl_bytes.to_vec(),
                Some(RdfFormat::Turtle),
                Overwrite::Allow,
            )
            .unwrap();
        env.env.add_ontology(ontology.clone()).unwrap();

        // Fresh cache should be reused.
        let reused = env
            .try_reuse_cached(&location, RefreshStrategy::UseCache)
            .unwrap();
        assert!(reused.is_some(), "fresh remote cache should be reused");

        // Age the cache past TTL and ensure reuse is skipped.
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let expired = env
            .try_reuse_cached(&location, RefreshStrategy::UseCache)
            .unwrap();
        assert!(expired.is_none(), "expired remote cache should refresh");
    }

    #[test]
    fn update_all_all_forces_refresh_even_when_cached() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let ttl_path = root.join("A.ttl");
        std::fs::write(
            &ttl_path,
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.com/A> a owl:Ontology .",
        )
        .unwrap();

        let config = Config::builder()
            .root(root.clone())
            .locations(vec![root.clone()])
            .includes(&["*.ttl"])
            .offline(true)
            .temporary(true)
            .use_cached_ontologies(CacheMode::Enabled)
            .build()
            .unwrap();
        let mut env = OntoEnv::init(config, true).unwrap();
        env.update_all(false).unwrap();

        // Capture original last_updated
        let id = env
            .resolve(ResolveTarget::Graph(
                NamedNode::new("http://example.com/A").unwrap(),
            ))
            .unwrap();
        let first_ts = env.ontologies().get(&id).unwrap().last_updated.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(1200));
        env.update_all(true).unwrap();

        let second_ts = env.ontologies().get(&id).unwrap().last_updated.unwrap();
        assert!(
            second_ts > first_ts,
            "update --all should force refresh even when cache is enabled"
        );
    }

    /// Build a temporary, offline OntoEnv with one ontology already registered,
    /// so transaction tests have non-trivial pre-state to roll back to.
    fn build_env_with_ontology() -> (OntoEnv, tempfile::TempDir, GraphIdentifier) {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let ttl_path = root.join("A.ttl");
        std::fs::write(
            &ttl_path,
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <http://example.com/tx-A> a owl:Ontology .",
        )
        .unwrap();
        let config = Config::builder()
            .root(root.clone())
            .offline(true)
            .temporary(true)
            .locations(vec![ttl_path.clone()])
            .build()
            .unwrap();
        let mut env = OntoEnv::init(config, true).unwrap();
        env.update_all(false).unwrap();
        let id = env
            .env
            .ontologies()
            .keys()
            .next()
            .expect("seeded ontology should be registered")
            .clone();
        (env, tmp, id)
    }

    #[test]
    fn env_transaction_restore_undoes_direct_mutations() {
        let (mut env, _tmp, _id) = build_env_with_ontology();
        let pre_onts = env.env.ontologies().len();
        let pre_dep_nodes = env.dependency_graph.node_count();
        let pre_index = env.dependency_graph_index.len();
        let pre_failed = env.failed_resolutions.len();

        let snapshot = EnvTransaction::snapshot(&env);

        // Mutate every field the snapshot covers.
        env.env = Environment::new();
        env.dependency_graph = DiGraph::new();
        env.dependency_graph_index.clear();
        env.failed_resolutions
            .insert(NamedNode::new_unchecked("http://example.com/tx-fake"));
        assert_eq!(env.env.ontologies().len(), 0);

        snapshot.restore(&mut env);

        assert_eq!(env.env.ontologies().len(), pre_onts);
        assert_eq!(env.dependency_graph.node_count(), pre_dep_nodes);
        assert_eq!(env.dependency_graph_index.len(), pre_index);
        assert_eq!(env.failed_resolutions.len(), pre_failed);
    }

    #[test]
    fn with_env_transaction_rolls_back_on_err() {
        let (mut env, _tmp, _id) = build_env_with_ontology();
        let pre_onts = env.env.ontologies().len();
        let pre_dep_nodes = env.dependency_graph.node_count();
        let pre_index = env.dependency_graph_index.len();

        let result: Result<()> = env.with_env_transaction(|s| {
            s.env = Environment::new();
            s.dependency_graph = DiGraph::new();
            s.dependency_graph_index.clear();
            s.failed_resolutions
                .insert(NamedNode::new_unchecked("http://example.com/tx-fail"));
            Err(anyhow!("simulated mid-operation failure"))
        });

        assert!(result.is_err());
        assert_eq!(env.env.ontologies().len(), pre_onts);
        assert_eq!(env.dependency_graph.node_count(), pre_dep_nodes);
        assert_eq!(env.dependency_graph_index.len(), pre_index);
        assert!(
            !env.failed_resolutions
                .contains(&NamedNode::new_unchecked("http://example.com/tx-fail")),
            "failed_resolutions mutation should have rolled back"
        );
    }

    #[test]
    fn with_env_transaction_commits_on_ok() {
        let (mut env, _tmp, _id) = build_env_with_ontology();
        let marker = NamedNode::new_unchecked("http://example.com/tx-commit");

        let result: Result<()> = env.with_env_transaction(|s| {
            s.failed_resolutions.insert(marker.clone());
            Ok(())
        });

        assert!(result.is_ok());
        assert!(
            env.failed_resolutions.contains(&marker),
            "Ok-return should commit mutations"
        );
    }
}
