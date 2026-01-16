//! Defines the main OntoEnv API struct and its methods for managing the ontology environment.
//! This includes loading, saving, updating, and querying the environment.

use crate::config::Config;
use crate::consts::IMPORTS;
use crate::doctor::{
    ConflictingPrefixes, Doctor, DuplicateOntology, OntologyDeclaration, OntologyProblem,
};
use crate::environment::Environment;
use crate::options::{Overwrite, RefreshStrategy};
use crate::transform;
use crate::ToUriString;
use crate::{EnvironmentStatus, FailedImport};
use chrono::prelude::*;
use oxigraph::model::{Dataset, Graph, NamedNode, NamedNodeRef, NamedOrBlankNodeRef, TripleRef};
use oxigraph::store::Store;
use petgraph::visit::EdgeRef;
use regex::Regex;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::path::PathBuf;

use crate::io::GraphIO;
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use anyhow::{anyhow, Result};
use blake3;
use log::{debug, error, info, warn};
use petgraph::graph::{Graph as DiGraph, NodeIndex};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;

#[derive(Clone, Debug)]
struct PendingImport {
    location: OntologyLocation,
    overwrite: Overwrite,
    required: bool,
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
            return false;
        }
        if self.include.is_empty() {
            return true;
        }
        self.include.iter().any(|re| re.is_match(&iri))
    }
}

impl<'a> BatchScope<'a> {
    fn enter(env: &'a mut OntoEnv) -> Result<Self> {
        env.batch_state.begin();
        if let Err(err) = env.io.begin_batch() {
            env.batch_state.end();
            return Err(err);
        }
        Ok(Self {
            env,
            completed: false,
        })
    }

    fn run<T>(mut self, f: impl FnOnce(&mut OntoEnv) -> Result<T>) -> Result<T> {
        let result = f(self.env);
        let end_result = self.env.io.end_batch();
        self.env.batch_state.end();
        self.completed = true;
        match (result, end_result) {
            (Ok(value), Ok(())) => Ok(value),
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

pub struct OntoEnv {
    env: Environment,
    io: Box<dyn GraphIO>,
    dependency_graph: DiGraph<GraphIdentifier, (), petgraph::Directed>,
    config: Config,
    failed_resolutions: HashSet<NamedNode>,
    batch_state: BatchState,
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

impl OntoEnv {
    // Constructors
    fn new(env: Environment, io: Box<dyn GraphIO>, config: Config) -> Self {
        Self {
            env,
            io,
            config,
            dependency_graph: DiGraph::new(),
            failed_resolutions: HashSet::new(),
            batch_state: BatchState::default(),
        }
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

    /// Ensure file locations are anchored to the OntoEnv root; leave other variants untouched.
    fn resolve_location(&self, location: OntologyLocation) -> OntologyLocation {
        match location {
            OntologyLocation::File(p) if p.is_relative() => {
                OntologyLocation::File(self.resolve_path(&p))
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

        if let Some(found_root) = find_ontoenv_root_from(&root) {
            let ontoenv_dir = found_root.join(".ontoenv");
            if ontoenv_dir.exists() {
                return Self::load_from_directory(found_root, read_only);
            }
        }

        let ontoenv_dir = root.join(".ontoenv");
        if ontoenv_dir.exists() {
            return Self::load_from_directory(root, read_only);
        }

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
            // Don't load as read_only
            Self::load_from_directory(root, false)
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
            // overwrite should be false, but init will create it.
            Self::init(config, false)
        }
    }

    /// Creates a new offline OntoEnv that searches for ontologies in the current directory.
    /// If an environment already exists, it will be loaded.
    /// The environment will be persisted to disk in the `.ontoenv` directory.
    pub fn new_offline() -> Result<Self> {
        // Convenience ctor for local dev without network access.
        if let Some(root) = find_ontoenv_root() {
            // Don't load as read_only
            Self::load_from_directory(root, false)
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
            // overwrite should be false, but init will create it.
            Self::init(config, false)
        }
    }

    /// Creates a new offline OntoEnv with no local search paths.
    /// If an environment already exists, it will be loaded.
    /// The environment will be persisted to disk in the `.ontoenv` directory.
    pub fn new_offline_no_search() -> Result<Self> {
        // Offline mode with no search paths to avoid filesystem scans.
        if let Some(root) = find_ontoenv_root() {
            // Don't load as read_only
            Self::load_from_directory(root, false)
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
            // overwrite should be false, but init will create it.
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

        let mut ontoenv = Self::new(Environment::new(), io, config);
        let _ = ontoenv.update_all(false)?;
        Ok(ontoenv)
    }

    /// Creates a new OntoEnv using a caller-provided GraphIO implementation.
    /// This is useful for embedding OntoEnv into applications with custom graph storage.
    pub fn new_with_graph_io(config: Config, io: Box<dyn GraphIO>) -> Result<Self> {
        // Plug in a custom GraphIO implementation and run initial update pass.
        let mut ontoenv = Self::new(Environment::new(), io, config);
        let _ = ontoenv.update_all(false)?;
        Ok(ontoenv)
    }

    /// returns the graph identifier for the given resolve target, if it exists
    pub fn resolve(&self, target: ResolveTarget) -> Option<GraphIdentifier> {
        // Map a location or graph IRI to the canonical GraphIdentifier.
        match target {
            ResolveTarget::Location(location) => self
                .env
                .get_ontology_by_location(&location)
                .map(|ont| ont.id().clone()),
            ResolveTarget::Graph(iri) => self
                .env
                .get_ontology_by_name(iri.as_ref())
                .map(|ont| ont.id().clone()),
        }
    }

    /// Saves the current environment to the .ontoenv directory.
    pub fn save_to_directory(&self) -> Result<()> {
        // Persist config, environment, and dependency graph to `.ontoenv`.
        if self.config.temporary {
            warn!("Cannot save a temporary environment");
            return Ok(());
        }
        let ontoenv_dir = self.config.root.join(".ontoenv");
        info!("Saving ontology environment to: {ontoenv_dir:?}");
        std::fs::create_dir_all(&ontoenv_dir)?;

        // Save the environment configuration
        let config_path = ontoenv_dir.join("ontoenv.json");
        let config_str = serde_json::to_string_pretty(&self.config)?;
        let mut file = std::fs::File::create(config_path)?;
        file.write_all(config_str.as_bytes())?;

        // Save the environment
        let env_path = ontoenv_dir.join("environment.json");
        let env_str = serde_json::to_string_pretty(&self.env)?;
        let mut file = std::fs::File::create(env_path)?;
        file.write_all(env_str.as_bytes())?;
        let graph_path = ontoenv_dir.join("dependency_graph.json");
        let graph_str = serde_json::to_string_pretty(&self.dependency_graph)?;
        let mut file = std::fs::File::create(graph_path)?;
        file.write_all(graph_str.as_bytes())?;

        Ok(())
    }

    pub fn new_temporary(&self) -> Result<Self> {
        // Clone the environment into an in-memory store for safe experimentation.
        let io: Box<dyn GraphIO> = Box::new(crate::io::MemoryGraphIO::new(
            self.config.offline,
            self.config.strict,
        )?);
        Ok(Self::new(self.env.clone(), io, self.config.clone()))
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
        // Load persisted config, environment, and dependency graph from disk.
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

        // Load the dependency graph
        let graph_path = ontoenv_dir.join("dependency_graph.json");
        let file = std::fs::File::open(graph_path)?;
        let reader = BufReader::new(file);
        let dependency_graph: DiGraph<GraphIdentifier, (), petgraph::Directed> =
            serde_json::from_reader(reader)?;

        // Load the environment
        let env_path = ontoenv_dir.join("environment.json");
        let file = std::fs::File::open(env_path)?;
        let reader = BufReader::new(file);
        // TODO: clean up the locations field loading
        let mut env: Environment = serde_json::from_reader(reader)?;
        env.normalize_file_locations(&config.root);

        // Initialize the IO to the persistent graph type. We know that it exists because we
        // are loading from a directory
        let mut io: Box<dyn GraphIO> = match read_only {
            true => Box::new(crate::io::ReadOnlyPersistentGraphIO::new(
                ontoenv_dir,
                config.offline,
            )?),
            false => Box::new(crate::io::PersistentGraphIO::new(
                ontoenv_dir,
                config.offline,
                config.strict,
            )?),
        };

        // copy the graphs from the persistent store to the memory store if we are a 'temporary'
        // environment
        if config.temporary {
            let mut new_io = Box::new(crate::io::MemoryGraphIO::new(
                config.offline,
                config.strict,
            )?);
            for ontology in env.ontologies().values() {
                let graph = io.get_graph(ontology.id())?;
                new_io.add_graph(ontology.id().clone(), graph)?;
            }
            io = new_io;
        }

        let mut ontoenv = OntoEnv {
            env,
            io,
            config,
            dependency_graph,
            failed_resolutions: HashSet::new(),
            batch_state: BatchState::default(),
        };

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
        // Compute on-disk status for CLI/diagnostic output.
        // get time modified of the self.store_path() directory
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
            config,
            failed_resolutions: HashSet::new(),
            batch_state: BatchState::default(),
        };

        if !ontoenv.config.use_cached_ontologies.is_enabled() {
            let _ = ontoenv.update_all(false)?;
        }

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

    fn add_with_options(
        &mut self,
        location: OntologyLocation,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
        update_dependencies: bool,
    ) -> Result<GraphIdentifier> {
        let location = self.resolve_location(location);
        self.with_io_batch(move |env| {
            env.add_with_options_inner(location, overwrite, refresh, update_dependencies)
        })
    }

    fn fetch_location(
        &mut self,
        location: OntologyLocation,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
    ) -> Result<FetchOutcome> {
        if let Some(existing_id) = self.try_reuse_cached(&location, refresh)? {
            self.batch_state.mark_seen(&location);
            return Ok(FetchOutcome::Reused(existing_id));
        }

        if !refresh.is_force() && self.batch_state.has_seen(&location) {
            if let Some(existing) = self.env.get_ontology_by_location(&location) {
                return Ok(FetchOutcome::Reused(existing.id().clone()));
            }
        }

        let ontology = self.io.add(location.clone(), overwrite)?;
        self.batch_state.mark_seen(&location);
        Ok(FetchOutcome::Loaded(Box::new(ontology)))
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

        self.save_to_directory()?;
        Ok(ids)
    }

    fn add_with_options_inner(
        &mut self,
        location: OntologyLocation,
        overwrite: Overwrite,
        refresh: RefreshStrategy,
        update_dependencies: bool,
    ) -> Result<GraphIdentifier> {
        // Reset per-call error tracking so stale failures do not leak across operations.
        self.failed_resolutions.clear();
        // Apply ontology filters early to keep store and env consistent.
        let ontology_filters = self.ontology_filters()?;
        self.prune_disallowed_ontologies(&ontology_filters, true)?;
        // Seed the import queue with the requested location and overwrite policy.
        let seeds = vec![(location.clone(), overwrite)];
        let (ontologies, reused_ids, errors) =
            self.process_import_queue(seeds, refresh, update_dependencies)?;
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
            let mut base = format!(
                "Failed to add ontology for location {}",
                location
            );
            if !errors.is_empty() {
                base.push_str(": ");
                base.push_str(&errors.join("; "));
            }
            anyhow!(base)
        })
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
        let seeds: Vec<(OntologyLocation, Overwrite)> = updated_files
            .into_iter()
            .map(|loc| (loc, Overwrite::Allow))
            .collect();
        // Force refresh when requested, otherwise reuse cached where possible.
        let refresh = if all {
            RefreshStrategy::Force
        } else {
            RefreshStrategy::UseCache
        };
        let (ontologies, reused_ids, _errors) = self.process_import_queue(seeds, refresh, true)?;

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
                        let source_modified = self
                            .io
                            .source_last_modified(ontology.id())
                            .unwrap_or(Utc::now());
                        source_modified > last_updated
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
        seeds: Vec<(OntologyLocation, Overwrite)>,
        refresh: RefreshStrategy,
        include_imports: bool,
    ) -> Result<(Vec<Ontology>, Vec<GraphIdentifier>, Vec<String>)> {
        // Use a BFS-style queue to load ontologies and (optionally) their imports.
        let strict = self.config.strict;
        let mut queue: VecDeque<PendingImport> = seeds
            .into_iter()
            .map(|(location, overwrite)| PendingImport {
                location,
                overwrite,
                required: strict,
            })
            .collect();
        // Track locations to prevent cycles and duplicate fetches.
        let mut seen: HashSet<OntologyLocation> = HashSet::new();
        let mut fetched: Vec<Ontology> = Vec::new();
        // Preserve insertion order of touched ids for stable outputs.
        let mut touched_ids: Vec<GraphIdentifier> = Vec::new();
        let mut touched_set: HashSet<GraphIdentifier> = HashSet::new();
        let mut errors: Vec<String> = Vec::new();

        let mut record_id = |id: &GraphIdentifier| {
            if touched_set.insert(id.clone()) {
                touched_ids.push(id.clone());
            }
        };

        while let Some(job) = queue.pop_front() {
            if !seen.insert(job.location.clone()) {
                continue;
            }

            match self.fetch_location(job.location.clone(), job.overwrite, refresh) {
                Ok(FetchOutcome::Loaded(ontology)) => {
                    let ontology = *ontology;
                    let imports = ontology.imports.clone();
                    let id = ontology.id().clone();
                    if include_imports {
                        // Queue imported ontologies to build a complete closure.
                        for import in imports {
                            self.queue_import_location(&import, &mut queue, self.config.strict)?;
                        }
                    }
                    fetched.push(ontology);
                    record_id(&id);
                }
                Ok(FetchOutcome::Reused(id)) => {
                    // Reused ontologies still contribute to the dependency graph.
                    record_id(&id);
                    if include_imports {
                        if let Ok(existing) = self.get_ontology(&id) {
                            // Preserve traversal by pulling imports from cached metadata.
                            for import in existing.imports {
                                self.queue_import_location(
                                    &import,
                                    &mut queue,
                                    self.config.strict,
                                )?;
                            }
                        }
                    }
                }
                Err(err) => {
                    let err_str = err.to_string();
                    let enriched = format!("Failed to load ontology {}: {}", job.location, err_str);
                    if job.required {
                        return Err(anyhow!(enriched));
                    }
                    // Non-strict mode records errors but continues processing.
                    warn!("{}", enriched);
                    errors.push(enriched);
                    if let OntologyLocation::Url(url) = &job.location {
                        if let Ok(node) = NamedNode::new(url.clone()) {
                            self.failed_resolutions.insert(node);
                        }
                    }
                }
            }
        }

        Ok((fetched, touched_ids, errors))
    }

    fn queue_import_location(
        &mut self,
        import: &NamedNode,
        queue: &mut VecDeque<PendingImport>,
        strict: bool,
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
                queue.push_back(PendingImport {
                    location: loc.clone(),
                    overwrite: Overwrite::Preserve,
                    required: strict,
                });
                return Ok(());
            }
        }

        // Otherwise, treat the IRI as a location and enqueue it for retrieval.
        match OntologyLocation::from_str(iri) {
            Ok(loc) => queue.push_back(PendingImport {
                location: loc,
                overwrite: Overwrite::Preserve,
                required: strict,
            }),
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
            let resolved = self.resolve_path(location);
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
                    files.insert(OntologyLocation::File(resolved.clone()));
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
                    files.insert(OntologyLocation::File(entry.path().to_path_buf()));
                }
            }
        }
        Ok(files.into_iter().collect())
    }

    fn add_ids_to_dependency_graph(&mut self, ids: Vec<GraphIdentifier>) -> Result<()> {
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
            // it and we're in strict mode, return an error. Otherwise just skip it
            let ontology = match self.env.get_ontology(&graphid) {
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
        Ok(())
    }

    fn rebuild_dependency_graph(&mut self) -> Result<()> {
        let ids: Vec<GraphIdentifier> = self.env.ontologies().keys().cloned().collect();
        self.dependency_graph = DiGraph::new();
        if ids.is_empty() {
            return Ok(());
        }
        self.add_ids_to_dependency_graph(ids)
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
        // Traverse imports with optional depth limit to build a dependency closure.
        let mut closure: HashSet<GraphIdentifier> = HashSet::new();
        let mut stack: VecDeque<(GraphIdentifier, i32)> = VecDeque::new();

        // TODO: how to handle a graph which is not in the environment?

        stack.push_back((id.clone(), 0));
        while let Some((graph, depth)) = stack.pop_front() {
            if !closure.insert(graph.clone()) {
                continue;
            }

            if recursion_depth >= 0 && depth >= recursion_depth {
                continue;
            }

            let ontology = self
                .ontologies()
                .get(&graph)
                .ok_or_else(|| anyhow!("Ontology {} not found", graph.to_uri_string()))?;
            for import in &ontology.imports {
                // get graph identifier for import
                let import = match self.env.get_ontology_by_name(import.into()) {
                    Some(imp) => imp.id().clone(),
                    None => {
                        if self.config.strict {
                            return Err(anyhow::anyhow!("Import not found: {}", import));
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
        // remove the original graph from the closure
        let mut closure: Vec<GraphIdentifier> = closure.into_iter().collect();
        if let Some(pos) = closure.iter().position(|x| x == id) {
            let root = closure.remove(pos);
            closure.insert(0, root);
        }
        info!("Dependency closure for {:?}: {:?}", id, closure.len());
        Ok(closure)
    }

    pub fn get_union_graph<'a, I>(
        &self,
        graph_ids: I,
        rewrite_sh_prefixes: Option<bool>,
        remove_owl_imports: Option<bool>,
    ) -> Result<UnionGraph>
    where
        I: IntoIterator<Item = &'a GraphIdentifier>,
    {
        // Merge multiple graphs into a dataset with optional cleanup transforms.
        let mut graph_ids: Vec<GraphIdentifier> = graph_ids.into_iter().cloned().collect();

        // TODO: figure out failed imports
        if graph_ids.is_empty() {
            return Err(anyhow!("No graphs found"));
        }

        // Identify a root by finding a graph that is not imported by any others.
        // Ensure the root ontology is first, even if callers pass an unordered collection.
        let mut imported: HashSet<GraphIdentifier> = HashSet::new();
        for graph_id in &graph_ids {
            if let Ok(ontology) = self.get_ontology(graph_id) {
                for import in &ontology.imports {
                    if let Some(imp) = self.env.get_ontology_by_name(import.into()) {
                        let imp_id = imp.id().clone();
                        if graph_ids.contains(&imp_id) {
                            imported.insert(imp_id);
                        }
                    }
                }
            }
        }
        let mut root_idx: Option<usize> = None;
        for (idx, graph_id) in graph_ids.iter().enumerate() {
            if !imported.contains(graph_id) {
                root_idx = Some(idx);
                break;
            }
        }
        if let Some(idx) = root_idx {
            if idx != 0 {
                let root = graph_ids.remove(idx);
                graph_ids.insert(0, root);
            }
        }

        // Merge all named graphs into a single dataset in IO order.
        let mut dataset = self.io.union_graph(&graph_ids);
        let root_ontology = NamedOrBlankNodeRef::NamedNode(graph_ids[0].name());

        // Merge namespace maps so downstream tools can re-materialize prefixes.
        let mut namespace_map = HashMap::new();
        for graph_id in &graph_ids {
            let ontology = self.get_ontology(graph_id)?;
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
            transform::rewrite_sh_prefixes_dataset(&mut dataset, root_ontology);
        }
        // remove owl:imports
        if remove_owl_imports.unwrap_or(true) {
            let to_remove: Vec<NamedNodeRef> = graph_ids.iter().map(|id| id.into()).collect();
            transform::remove_owl_imports(&mut dataset, Some(&to_remove));
        }
        // Collapse ontology declarations onto the chosen root.
        transform::remove_ontology_declarations(&mut dataset, root_ontology);
        Ok(UnionGraph {
            dataset,
            graph_ids,
            failed_imports: None, // TODO: Populate this correctly
            namespace_map,
        })
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
        let mut union = self.get_union_graph(&closure, Some(false), Some(false))?;

        let root_nb = NamedOrBlankNodeRef::NamedNode(root);
        // Apply transforms with the requested root.
        transform::rewrite_sh_prefixes_dataset(&mut union.dataset, root_nb);
        transform::remove_owl_imports(&mut union.dataset, None);
        transform::remove_ontology_declarations(&mut union.dataset, root_nb);

        // Flatten dataset into a single graph, ignoring named graph labels.
        let mut graph = Graph::new();
        for quad in union.dataset.iter() {
            // Drop owl:imports on non-root subjects to prevent retaining inner edges in cycles.
            if quad.predicate == IMPORTS && quad.subject != root_nb {
                continue;
            }
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

    pub fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        // Delegate graph retrieval to the IO backend.
        self.io.get_graph(id)
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
}
