//! Experimental atomic publication of OntoEnv versions backed by immutable RDF5D files.
//!
//! Enable `transactional-snapshots`. This API uses a separate
//! `.ontoenv-snapshots` directory and does not migrate or modify legacy stores.
//! Graph changes are staged over an immutable base. An atomically replaced
//! `CURRENT` file selects an immutable manifest bundling graph references,
//! metadata, aliases and configuration. No database or reader recovery is needed.
//! Published objects are retained indefinitely until online retention is added.

use crate::config::Config;
use crate::environment::Environment;
use crate::io::{
    fetch_source, oxigraph_object_to_r5term, oxigraph_subject_to_r5term, parse_ontology_source,
};
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use oxigraph::io::RdfFormat;
use oxigraph::model::{Graph, NamedNodeRef};
use rdf5d::{Quint, Snapshot, StreamingWriter, WriterOptions};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DIRECTORY: &str = ".ontoenv-snapshots";
const SCHEMA_VERSION: u32 = 2;
const CURRENT: &str = "CURRENT";

#[derive(Debug, Serialize, Deserialize)]
struct Head {
    schema_version: u32,
    manifest: String,
    manifest_hash: String,
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
}

impl EnvironmentSnapshot {
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

    /// Copy one graph from this generation; empty and missing graphs differ.
    pub fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
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

    /// Compare with committed head without changing this reader's snapshot.
    pub fn is_current(&self) -> Result<bool> {
        let head = read_head(&self.directory)?;
        Ok(head.info.environment_id == self.current.info.environment_id
            && head.info.generation == self.current.info.generation)
    }

    /// Validate and swap the latest snapshot as a unit. Errors preserve the old one.
    /// Returns whether the selected committed generation changed.
    pub fn refresh(&mut self) -> Result<bool> {
        let generation = read_head(&self.directory)?;
        if generation.info.environment_id == self.current.info.environment_id
            && generation.info.generation == self.current.info.generation
        {
            return Ok(false);
        }
        let next = open_generation(generation, &self.directory)?;
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
    _writer_lock: File,
}

impl SnapshotWriter {
    /// Create an experimental environment, refusing to recreate existing data.
    /// A legacy `.ontoenv` directory may coexist; it is not imported automatically.
    pub fn create(mut config: Config) -> Result<Self> {
        if config.temporary || config.external_graph_store.is_some() {
            bail!("Snapshot storage requires persistent, managed graph storage");
        }
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
        let mut environment = Environment::new();
        environment.set_default_policy(&config.resolution_policy)?;
        let (object_id, object_length, rdf) =
            write_object(initialization.path(), None, &BTreeMap::new())?;
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
        fs::rename(initialization.path(), &directory)?;
        let _initialized_path = initialization.keep();
        sync_directory(&generation.config.root).context(
            "Snapshot initialization was published, but syncing its parent failed; open to inspect",
        )?;
        let current = snapshot_from_generation(generation, rdf);
        Ok(Self {
            directory,
            current,
            needs_reopen: false,
            _writer_lock: writer_lock,
        })
    }

    /// Open the writer while existing readers retain their immutable snapshots.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = fs::canonicalize(root.as_ref())?;
        let writer_lock = acquire_writer_lock(&root)?;
        let directory = root.join(DIRECTORY);
        let current = open_head(&directory)?;
        Ok(Self {
            directory,
            current,
            needs_reopen: false,
            _writer_lock: writer_lock,
        })
    }

    /// The writer's last confirmed snapshot; staged changes are private.
    /// After a publication error it may lag durable head; reopen to reconcile.
    pub fn snapshot(&self) -> Arc<EnvironmentSnapshot> {
        Arc::clone(&self.current)
    }

    /// Start a copy-on-write update: only small catalog/config values are cloned.
    /// Untouched graph contents remain in the base snapshot's mapped RDF5D file.
    /// After a publication error, commits are rejected until the writer is reopened.
    pub fn begin_update(&mut self) -> SnapshotUpdate<'_> {
        let environment = self.current.environment.clone();
        let config = self.current.config.clone();
        SnapshotUpdate {
            writer: self,
            environment,
            config,
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
    environment: Environment,
    config: Config,
    changes: BTreeMap<String, Option<Graph>>,
}

impl SnapshotUpdate<'_> {
    /// Inspect candidate metadata without exposing mutable persistence state.
    pub fn environment(&self) -> &Environment {
        &self.environment
    }

    /// Edit persistent settings in the same transaction as graphs and aliases.
    /// Storage root, temporary mode and external-backend changes are rejected.
    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    // Only metadata derived from the candidate graph can enter the catalog.
    fn put_ontology(&mut self, ontology: Ontology, graph: Graph) -> Result<GraphIdentifier> {
        if ontology.location().is_none() {
            bail!("Cannot stage an ontology without a source location");
        }
        let id = ontology.id().clone();
        let name = id.name().as_str().to_owned();
        let old_ids: Vec<_> = self
            .environment
            .ontologies()
            .keys()
            .filter(|old| old.name() == id.name() || old.location() == id.location())
            .cloned()
            .collect();
        // Preserve explicit aliases when replacing metadata/location for an IRI.
        let aliases: Vec<_> = self
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
                self.changes.insert(old.name().as_str().to_owned(), None);
            }
            self.environment.remove_ontology(&old)?;
        }
        self.environment.add_ontology(ontology)?;
        for alias in aliases {
            self.environment.add_alias(&alias, &name)?;
        }
        self.changes.insert(name, Some(graph));
        Ok(id)
    }

    /// Replace an existing graph, deriving metadata from its new contents.
    /// Empty graphs retain their identifier but have no imports or RDF metadata.
    /// A different declared ontology IRI requires removing and adding the graph.
    pub fn replace_graph(&mut self, id: &GraphIdentifier, graph: Graph) -> Result<()> {
        if !self.environment.ontologies().contains_key(id) {
            bail!("Graph {id} is not present in the candidate");
        }
        let mut ontology = Ontology::from_graph(
            &graph,
            id.location().clone(),
            !graph.is_empty() && self.config.require_ontology_names,
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
        self.put_ontology(ontology, graph)?;
        Ok(())
    }

    /// Fetch and stage one source. Imports are validated at commit, not fetched
    /// recursively; stage their sources in the same update when strict is enabled.
    pub fn add_source(&mut self, location: OntologyLocation) -> Result<GraphIdentifier> {
        let location = normalize_source_location(location, &self.config.root);
        let (bytes, format) = fetch_source(&location, self.config.offline)?;
        self.add_from_bytes(location, bytes, format)
    }

    /// Parse and stage one source using the exact bytes whose hash is recorded.
    pub fn add_from_bytes(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
    ) -> Result<GraphIdentifier> {
        let location = normalize_source_location(location, &self.config.root);
        let parsed =
            parse_ontology_source(location, bytes, format, self.config.require_ontology_names)?;
        let graph = parsed.to_graph();
        self.put_ontology(parsed.ontology, graph)
    }

    /// Read staged contents, falling through to the mapped base for untouched graphs.
    pub fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        if !self.environment.ontologies().contains_key(id) {
            bail!("Graph {id} is not present in the candidate");
        }
        match self.changes.get(id.name().as_str()) {
            Some(Some(graph)) => Ok(graph.clone()),
            Some(None) => bail!("Graph {id} is deleted in the candidate"),
            None => self.writer.current.get_graph(id),
        }
    }

    /// Delete both graph contents and metadata. Returns whether the graph existed.
    pub fn remove(&mut self, id: &GraphIdentifier) -> Result<bool> {
        if self.environment.remove_ontology(id)?.is_none() {
            return Ok(false);
        }
        self.changes.insert(id.name().as_str().to_owned(), None);
        Ok(true)
    }

    /// Stage an alias without independently publishing catalog changes.
    pub fn add_alias(&mut self, alias: &str, canonical_iri: &str) -> Result<()> {
        self.environment.add_alias(alias, canonical_iri)
    }

    /// Remove an alias in the private candidate.
    pub fn remove_alias(&mut self, alias: &str) -> Result<()> {
        self.environment.remove_alias(alias)?;
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
        let (object_id, object_length, rdf) = if self.changes.is_empty() {
            (
                base.object_id.clone(),
                base.object_length,
                Arc::clone(&base.rdf),
            )
        } else {
            write_object(&self.writer.directory, Some(base), &self.changes)?
        };
        self.environment
            .set_default_policy(&self.config.resolution_policy)?;
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
            environment: self.environment,
            config: self.config,
        };
        // Prepare the complete in-process snapshot before the publication point.
        let snapshot = snapshot_from_generation(generation, rdf);
        let record = snapshot.record();
        before_publish()?;
        self.writer.needs_reopen = true;
        publisher(&self.writer.directory, &record, Some(&base.info))?;
        self.writer.current = Arc::clone(&snapshot);
        self.writer.needs_reopen = false;
        Ok(snapshot)
    }

    fn validate(&self) -> Result<()> {
        if self.config.root != self.writer.current.config.root
            || self.config.temporary
            || self.config.external_graph_store.is_some()
        {
            bail!("An update cannot change storage root, temporary mode or graph backend");
        }
        if self.config.strict {
            for ontology in self.environment.ontologies().values() {
                for import in &ontology.imports {
                    if self
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
        if crate::policy::policy_from_name(&self.config.resolution_policy).is_none() {
            bail!(
                "Unknown resolution policy: {}",
                self.config.resolution_policy
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

fn acquire_writer_lock(root: &Path) -> Result<File> {
    let path = root.join(".ontoenv-snapshots.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    FileExt::try_lock_exclusive(&file)
        .with_context(|| format!("Another snapshot writer owns {}", path.display()))?;
    Ok(file)
}

fn read_head(directory: &Path) -> Result<Generation> {
    let bytes = fs::read(directory.join(CURRENT)).with_context(|| {
        if directory.join("manifest.redb").exists() {
            "This environment uses the earlier redb prototype; automatic migration is unsupported"
        } else {
            "Cannot read the selected OntoEnv version (CURRENT)"
        }
    })?;
    let head: Head = serde_json::from_slice(&bytes).context("Invalid CURRENT file")?;
    if head.schema_version != SCHEMA_VERSION {
        bail!("Unsupported CURRENT schema version {}", head.schema_version);
    }
    let valid_name = head
        .manifest
        .strip_prefix("manifest-")
        .and_then(|name| name.strip_suffix(".json"))
        .is_some_and(valid_id);
    if !valid_name {
        bail!("Invalid manifest name in CURRENT");
    }
    let bytes = fs::read(directory.join("manifests").join(&head.manifest))
        .context("Cannot read the manifest selected by CURRENT")?;
    if blake3::hash(&bytes).to_hex().as_str() != head.manifest_hash {
        bail!("Selected manifest checksum mismatch");
    }
    let generation: Generation = serde_json::from_slice(&bytes)?;
    validate_record(&generation)?;
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
    // Nothing is pruned: the selected manifest and object remain available after
    // CURRENT closes, even if a writer publishes another version in between.
    open_generation(read_head(directory)?, directory)
}

fn open_generation(generation: Generation, directory: &Path) -> Result<Arc<EnvironmentSnapshot>> {
    let path = directory
        .join("objects")
        .join(&generation.object_id)
        .join("store.r5tu");
    if fs::metadata(&path)?.len() != generation.object_length {
        bail!("Snapshot object length does not match its committed manifest");
    }
    let rdf = Arc::new(Snapshot::open(&path)?);
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
    Ok(snapshot_from_generation(generation, rdf))
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
    mut at: impl FnMut(PublicationPhase) -> Result<()>,
) -> Result<()> {
    validate_record(generation)?;
    // The stable exclusive writer lock serializes this check and replacement.
    // This is not a compare-and-swap for writers bypassing that lock.
    if let Some(expected) = expected {
        let current = read_head(directory)?;
        if current.info.environment_id != expected.environment_id
            || current.info.generation != expected.generation
        {
            bail!("Committed head changed since update began; reopen the writer");
        }
    } else if directory.join(CURRENT).try_exists()? {
        bail!("Unexpected snapshot manifest head; reopen the writer");
    }
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
    base: Option<&EnvironmentSnapshot>,
    changes: &BTreeMap<String, Option<Graph>>,
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
        for group in base.rdf.file().enumerate_all()? {
            if changes.contains_key(&group.graphname) {
                continue;
            }
            for triple in base.rdf.file().oxigraph_triples(group.gid)? {
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
    let rdf = Arc::new(Snapshot::open(&path)?);
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
                .get_graph(&id)?
                .iter()
                .any(|t| t.object.to_string() == expected));
            assert!(retained
                .get_graph(&id)?
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
                .get_graph(&id)?
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
                .get_graph(&id)?
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
                .get_graph(&id)?
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
