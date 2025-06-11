//! Defines the main OntoEnv API struct and its methods for managing the ontology environment.
//! This includes loading, saving, updating, and querying the environment.

use crate::config::Config;
use crate::doctor::{ConflictingPrefixes, Doctor, DuplicateOntology, OntologyDeclaration};
use crate::environment::Environment;
use crate::transform;
use crate::{EnvironmentStatus, FailedImport};
use chrono::prelude::*;
use oxigraph::model::{Dataset, Graph, NamedNode, NamedNodeRef, SubjectRef};
use oxigraph::store::Store;
use petgraph::visit::EdgeRef;
use std::io::{BufReader, Write};
use std::path::Path;
use std::path::PathBuf;

use crate::io::GraphIO;
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use anyhow::Result;
use log::{error, info, warn};
use petgraph::graph::{Graph as DiGraph, NodeIndex};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;

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
        self.dataset.len()
    }

    /// Returns the union of all namespace maps from the ontologies in the graph.
    pub fn get_namespace_map(&self) -> &HashMap<String, String> {
        &self.namespace_map
    }
}

pub struct Stats {
    pub num_triples: usize,
    pub num_graphs: usize,
    pub num_ontologies: usize,
}

pub struct OntoEnv {
    env: Environment,
    io: Box<dyn GraphIO>,
    dependency_graph: DiGraph<GraphIdentifier, (), petgraph::Directed>,
    config: Config,
}

impl std::fmt::Debug for OntoEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // print config
        write!(f, "OntoEnv {{\n")?;
        write!(f, "  config: {:?},\n", self.config)?;
        write!(f, "  env: {:?},\n", self.env)?;
        write!(f, "  dependency_graph: {:?},\n", self.dependency_graph)?;
        write!(f, "  io: {:?},\n", self.io.io_type())?;
        write!(f, "}}")?;
        Ok(())
    }
}

impl OntoEnv {
    fn new(env: Environment, io: Box<dyn GraphIO>, config: Config) -> Self {
        Self {
            env,
            io,
            config,
            dependency_graph: DiGraph::new(),
        }
    }

    pub fn new_from_store(strict: bool, offline: bool, store: Store) -> Result<Self> {
        let io = Box::new(crate::io::ExternalStoreGraphIO::new(
            store, offline, strict,
        ));
        let root = std::env::current_dir()?;
        let _includes = vec!["*.ttl", "*.xml", "*.n3"];
        let config = Config::new_with_default_matches(
            root,
            None::<Vec<PathBuf>>,
            false,
            false,
            strict,
            offline,
        )?;

        Ok(Self::new(Environment::new(), io, config))
    }

    pub fn io(&self) -> &Box<dyn GraphIO> {
        &self.io
    }

    /// returns the graph identifier for the given resolve target, if it exists
    pub fn resolve(&self, target: ResolveTarget) -> Option<GraphIdentifier> {
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

    pub fn stats(&self) -> Result<Stats> {
        let store_stats = self.io.size()?;
        Ok(Stats {
            num_triples: store_stats.num_triples,
            num_graphs: store_stats.num_graphs,
            num_ontologies: self.env.ontologies().len(),
        })
    }

    /// Saves the current environment to the .ontoenv directory.
    pub fn save_to_directory(&self) -> Result<()> {
        if self.config.temporary {
            warn!("Cannot save a temporary environment");
            return Ok(());
        }
        let ontoenv_dir = self.config.root.join(".ontoenv");
        info!("Saving ontology environment to: {:?}", ontoenv_dir);
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

    pub fn flush(&mut self) -> Result<()> {
        self.io.flush()
    }

    pub fn new_temporary(&self) -> Result<Self> {
        let io: Box<dyn GraphIO> = Box::new(crate::io::MemoryGraphIO::new(
            self.config.offline,
            self.config.strict,
        )?);
        Ok(Self::new(self.env.clone(), io, self.config.clone()))
    }

    /// Loads the environment from the .ontoenv directory.
    pub fn load_from_directory(root: PathBuf, read_only: bool) -> Result<Self> {
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
        let mut locations: HashMap<OntologyLocation, GraphIdentifier> = HashMap::new();
        for ontology in env.ontologies().values() {
            locations.insert(ontology.location().unwrap().clone(), ontology.id().clone());
        }
        env.locations = locations;

        // Initialize the IO to the persistent graph type. We know that it exists because we
        // are loading from a directory
        let mut io: Box<dyn GraphIO> = match read_only {
            true => Box::new(crate::io::ReadOnlyPersistentGraphIO::new(
                ontoenv_dir.into(),
                config.offline,
            )?),
            false => Box::new(crate::io::PersistentGraphIO::new(
                ontoenv_dir.into(),
                config.offline,
                config.strict,
            )?),
        };

        // copy the graphs from the persistent store to the memory store if we are a 'temporary'
        // environment
        if config.temporary {
            let mut new_io =
                Box::new(crate::io::MemoryGraphIO::new(config.offline, config.strict)?);
            for ontology in env.ontologies().values() {
                let graph = io.get_graph(ontology.id())?;
                new_io.add_graph(ontology.id().clone(), graph)?;
            }
            io = new_io;
        }

        Ok(OntoEnv {
            env,
            io,
            config,
            dependency_graph,
        })
    }

    /// Calculates and returns the environment status
    pub fn status(&self) -> Result<EnvironmentStatus> {
        // get time modified of the self.store_path() directory
        let last_updated: DateTime<Utc> = std::fs::metadata(self.config.root.join(".ontoenv"))?
            .modified()?
            .into();
        // get the size of the .ontoenv directory on disk
        //let size = self.get_store_size()?;
        let size = 999999;
        let num_ontologies = self.env.ontologies().len();
        Ok(EnvironmentStatus {
            exists: true,
            num_ontologies,
            last_updated: Some(last_updated),
            store_size: size,
        })
    }

    pub fn store_path(&self) -> Option<&Path> {
        self.io.store_location()
    }

    pub fn ontologies(&self) -> &HashMap<GraphIdentifier, Ontology> {
        self.env.ontologies()
    }

    /// Returns a table of metadata for the given graph
    pub fn graph_metadata(&self, id: &GraphIdentifier) -> HashMap<String, String> {
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

    /// Initializes a new API environment. If the environment directory already exists:
    /// - If `overwrite` is true, it will remove the existing directory and recreate it.
    /// - If `overwrite` is false, it will return an error.
    /// Returns a `Result` indicating success or failure
    pub fn init(config: Config, overwrite: bool) -> Result<Self> {
        let ontoenv_dir = config.root.join(".ontoenv");

        if !config.temporary && ontoenv_dir.exists() {
            if overwrite {
                info!(
                    "Directory exists and will be overwritten: {:?}",
                    ontoenv_dir
                );
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
            true => Box::new(crate::io::MemoryGraphIO::new(config.offline, config.strict)?),
            false => Box::new(crate::io::PersistentGraphIO::new(
                ontoenv_dir.into(),
                config.offline,
                config.strict,
            )?),
        };

        Ok(OntoEnv {
            env,
            io,
            dependency_graph: DiGraph::new(),
            config,
        })
    }

    /// Add the ontology from the given location to the environment,
    /// then add it to the dependency graph.
    pub fn add(&mut self, location: OntologyLocation, overwrite: bool) -> Result<GraphIdentifier> {
        let ont = self.io.add(location, overwrite)?;
        let id = ont.id().clone();
        self.env.add_ontology(ont);
        self.add_ids_to_dependency_graph(vec![id.clone()])?;
        self.save_to_directory()?;
        Ok(id)
    }

    /// Load all graphs from the search directories. There are several things that can happen:
    ///
    /// 1. files have been added from the search directories
    /// 2. files have been removed from the search directories
    /// 3. files have been updated in the search directories
    ///
    /// OntoEnv tries to do the least amount of work possible.
    ///
    /// First, it removes all ontologies which no longer appear in the search directories; it uses
    /// its internal index of ontologies to do this search.
    ///
    /// Next, it determines what new files have been added to the search directories. These are
    /// files whose locations do not appear in the internal ontology index. It also finds the files
    /// in the internal ontology index have been updated. It does this by comparing the last
    /// updated time of the file with the last updated time of the ontology in the index.
    ///
    /// Then, it reads all the new and updated files and adds them to the environment.
    ///
    /// Finally, it updates the dependency graph for all the updated ontologies.
    pub fn update(&mut self) -> Result<()> {
        // remove ontologies which are no longer present in the search directories
        for graphid in self.missing_ontologies() {
            self.io.remove(&graphid)?;
            self.env.remove_ontology(&graphid);
        }

        // now, find all the new and updated ontologies in the search directories
        // and add them to the environment
        let updated_files = self.get_updated_locations()?;

        // load all of these files into the environment
        let mut ontologies: Vec<Ontology> = vec![];
        for location in updated_files {
            // if 'strict' mode then fail on any errors when adding the ontology
            // otherwise just warn

            let result = self.io.add(location.clone(), true);
            if result.is_err() {
                if self.config.strict {
                    return Err(result.unwrap_err());
                } else {
                    warn!(
                        "Failed to read ontology file {}: {}",
                        location,
                        result.unwrap_err()
                    );
                    continue;
                }
            }

            let ontology = result.unwrap();
            ontologies.push(ontology);
        }

        let mut update_ids: Vec<GraphIdentifier> = Vec::new();
        // add the ontologies to the environment
        for ontology in ontologies {
            let id = ontology.id().clone();
            self.env.add_ontology(ontology);
            update_ids.push(id);
        }
        self.add_ids_to_dependency_graph(update_ids)?;
        self.save_to_directory()?;
        Ok(())
    }

    /// Returns a list of all ontologies from the environment which have been updated.
    fn get_updated_from_environment(&self) -> Vec<GraphIdentifier> {
        self.env
            .ontologies()
            .iter()
            .filter(|(_, ontology)| {
                // if the source modified is missing, then we assume it has been updated
                let source_modified = self
                    .io
                    .source_last_modified(ontology.id())
                    .unwrap_or(Utc::now());
                // if the ontology has no modified time, then we assume it has never been updated
                source_modified
                    > ontology
                        .last_updated
                        .unwrap_or(Utc.timestamp_opt(0, 0).unwrap())
            })
            .map(|(graphid, _)| graphid.clone())
            .collect()
    }

    /// Returns a list of all files in the environment which have been updated (added or changed)
    /// Does not return files that have been removed
    pub fn get_updated_locations(&self) -> Result<Vec<OntologyLocation>> {
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
            info!("Updating ontologies: {:?}", updated_ids);
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

    /// Lists all ontologies in the search directories which match
    /// the patterns
    pub fn find_files(&self) -> Result<Vec<OntologyLocation>> {
        let mut files = vec![];
        for location in &self.config.locations {
            // if location does not exist, skip it
            if !location.exists() {
                warn!("Location does not exist: {:?}", location);
                continue;
            }
            // if location is a file, add it to the list
            if location.is_file() && self.config.is_included(location) {
                files.push(OntologyLocation::File(location.clone()));
                continue;
            }
            for entry in walkdir::WalkDir::new(location) {
                let entry = entry?;
                if entry.file_type().is_file() && self.config.is_included(entry.path()) {
                    files.push(OntologyLocation::File(entry.path().to_path_buf()));
                }
            }
        }
        Ok(files)
    }

    fn add_ids_to_dependency_graph(&mut self, ids: Vec<GraphIdentifier>) -> Result<()> {
        // traverse the owl:imports closure and build the dependency graph
        let mut stack: VecDeque<GraphIdentifier> = ids.into();
        let mut seen: HashSet<GraphIdentifier> = HashSet::new();

        while let Some(graphid) = stack.pop_front() {
            info!("Building dependency graph for: {:?}", graphid);
            if seen.contains(&graphid) {
                continue;
            }
            seen.insert(graphid.clone());
            // get the ontology metadata record for this graph. If we don't have
            // it and we're in strict mode, return an error. Otherwise just skip it
            let ontology = match self.env.get_ontology(&graphid) {
                Some(ontology) => ontology,
                None => {
                    let msg = format!("Could not find ontology: {:?}", graphid);
                    if self.config.strict {
                        error!("{}", msg);
                        return Err(anyhow::anyhow!(msg));
                    } else {
                        warn!("{}", msg);
                        continue;
                    }
                }
            };
            let imports = &ontology.imports.clone();
            for import in imports {
                // check to see if we have a file defining this ontology first
                let location = if let Some(imp) = self.env.get_ontology_by_name(import.into()) {
                    // if we have already re-visited it, skip
                    if seen.contains(imp.id()) || stack.contains(imp.id()) {
                        continue;
                    }
                    imp.location()
                        .ok_or(anyhow::anyhow!(format!(
                            "Parsing imports: Ontology {} location not found",
                            imp
                        )))?
                        .clone()
                } else {
                    // otherwise, try to find the ontology by location
                    OntologyLocation::from_str(import.as_str())?
                };
                let imp = match self.io.add(location, false) {
                    Ok(imp) => {
                        let id = imp.id().clone();
                        self.env.add_ontology(imp);
                        id
                    }
                    Err(e) => {
                        if self.config.strict {
                            return Err(e);
                        } else {
                            warn!("Failed to read ontology file {}: {}", import.as_str(), e);
                            continue;
                        }
                    }
                };
                stack.push_back(imp);
            }
        }
        //
        // put the dependency graph into self.dependency_graph
        let mut indexes: HashMap<GraphIdentifier, NodeIndex> = HashMap::new();
        let mut graph: DiGraph<GraphIdentifier, (), petgraph::Directed> = DiGraph::new();
        // add all ontologies in self.ontologies to the graph
        for ontology in self.env.ontologies().keys() {
            let index = graph.add_node(ontology.clone());
            indexes.insert(ontology.clone(), index);
        }
        // traverse the ontologies and add edges to the graph
        for ontology in self.env.ontologies().keys() {
            let index = indexes.get(ontology).unwrap();
            let ont = match self.env.ontologies().get(ontology) {
                Some(ont) => ont,
                None => {
                    error!("Ontology not found: {:?}", ontology);
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
                        warn!("Import not found: {}", import);
                        continue;
                    }
                };
                let import_index = indexes.get(graph_id).unwrap();
                graph.add_edge(*index, *import_index, ());
            }
        }
        self.dependency_graph = graph;
        Ok(())
    }

    /// Returns a list of issues with the environment
    pub fn doctor(&self) {
        let mut doctor = Doctor::new();
        doctor.add_check(Box::new(DuplicateOntology {}));
        doctor.add_check(Box::new(OntologyDeclaration {}));
        doctor.add_check(Box::new(ConflictingPrefixes {}));

        let problems = doctor.run(self).unwrap();

        // for each problem, print two columns. The first column is the message
        // and the second column is a list of locations for that problem. The locations
        // should be stacked on top of one another
        let mut messages: HashMap<String, Vec<String>> = HashMap::new();
        for problem in problems {
            let message = problem.message;
            let locations: Vec<String> = problem.locations.iter().map(|l| l.to_string()).collect();
            messages.entry(message).or_default().extend(locations);
        }

        // print the messages
        for (message, locations) in messages {
            println!("Problem: {}", message);
            for location in locations {
                println!("  - {}", location);
            }
        }
    }

    /// Returns the names of all graphs within the dependency closure of the provided graph
    pub fn get_dependency_closure(&self, id: &GraphIdentifier) -> Result<Vec<GraphIdentifier>> {
        let mut closure: HashSet<GraphIdentifier> = HashSet::new();
        let mut stack: VecDeque<GraphIdentifier> = VecDeque::new();

        // TODO: how to handle a graph which is not in the environment?

        stack.push_back(id.clone());
        while let Some(graph) = stack.pop_front() {
            closure.insert(graph.clone());
            let ontology = self
                .ontologies()
                .get(&graph)
                .ok_or(anyhow::anyhow!("Ontology not found"))?;
            for import in &ontology.imports {
                // get graph identifier for import
                let import = match self.env.get_ontology_by_name(import.into()) {
                    Some(imp) => imp.id().clone(),
                    None => {
                        if self.config.strict {
                            return Err(anyhow::anyhow!("Import not found: {}", import));
                        }
                        warn!("Import not found: {}", import);
                        continue;
                    }
                };
                if !closure.contains(&import) {
                    stack.push_back(import);
                }
            }
        }
        // remove the original graph from the closure
        closure.remove(id);
        let mut closure: Vec<GraphIdentifier> = closure.into_iter().collect();
        closure.insert(0, id.clone());
        info!("Dependency closure for {:?}: {:?}", id, closure.len());
        Ok(closure)
    }

    pub fn get_union_graph(
        &self,
        graph_ids: &[GraphIdentifier],
        rewrite_sh_prefixes: Option<bool>,
        remove_owl_imports: Option<bool>,
    ) -> Result<UnionGraph> {
        // TODO: figure out failed imports
        let mut dataset = self.io.union_graph(graph_ids);
        let first_id = graph_ids
            .first()
            .ok_or(anyhow::anyhow!("No graphs found"))?;
        let root_ontology: SubjectRef = SubjectRef::NamedNode(first_id.name());

        let mut namespace_map = HashMap::new();
        for graph_id in graph_ids {
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
            transform::rewrite_sh_prefixes(&mut dataset, root_ontology);
        }
        // remove owl:imports
        if remove_owl_imports.unwrap_or(true) {
            let to_remove: Vec<NamedNodeRef> = graph_ids.iter().map(|id| id.into()).collect();
            transform::remove_owl_imports(&mut dataset, Some(&to_remove));
        }
        transform::remove_ontology_declarations(&mut dataset, root_ontology);
        Ok(UnionGraph {
            dataset,
            graph_ids: graph_ids.to_vec(),
            failed_imports: None, // TODO: Populate this correctly
            namespace_map,
        })
    }

    pub fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        self.io.get_graph(id)
    }

    pub fn get_ontology(&self, id: &GraphIdentifier) -> Result<Ontology> {
        self.env
            .get_ontology(id)
            .ok_or(anyhow::anyhow!("Ontology not found"))
    }

    /// Returns a list of all ontologies that depend on the given ontology
    pub fn get_dependents(&self, id: &NamedNode) -> Result<Vec<GraphIdentifier>> {
        // find all nodes in the dependency_graph which have an edge to the given node
        // and return the list of nodes
        let mut dependents: Vec<GraphIdentifier> = Vec::new();
        let node = self
            .env
            .get_ontology_by_name(id.into())
            .ok_or(anyhow::anyhow!("Ontology not found"))?;
        let index = self
            .dependency_graph
            .node_indices()
            .find(|i| self.dependency_graph[*i] == *node.id())
            .ok_or(anyhow::anyhow!("Node not found"))?;
        for edge in self
            .dependency_graph
            .edges_directed(index, petgraph::Direction::Incoming)
        {
            let dependent = self.dependency_graph[edge.source()].clone();
            dependents.push(dependent);
        }
        Ok(dependents)
    }

    /// Returns the GraphViz dot representation of the dependency graph
    pub fn dep_graph_to_dot(&self) -> Result<String> {
        self.rooted_dep_graph_to_dot(self.ontologies().keys().cloned().collect())
    }

    /// Return the GraphViz dot representation of the dependency graph
    /// rooted at the given graph
    pub fn rooted_dep_graph_to_dot(&self, roots: Vec<GraphIdentifier>) -> Result<String> {
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
                .ok_or(anyhow::anyhow!(format!(
                    "Listing ontologies: Ontology {} not found",
                    ontology
                )))?;
            for import in &ont.imports {
                let import = match self.env.get_ontology_by_name(import.into()) {
                    Some(imp) => imp.id().clone(),
                    None => {
                        error!("Import not found: {}", import);
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

        Ok(format!("digraph {{\nrankdir=LR;\n{:?}}}", dot))
    }

    /// Outputs a human-readable dump of the environment, including all ontologies
    /// and their metadata and imports
    pub fn dump(&self, contains: Option<&str>) {
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
            println!("┌ Ontology: {}", name);
            for ontology in group {
                let g = self.io.get_graph(ontology.id()).unwrap();
                println!("├─ Location: {}", ontology.location().unwrap());
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
                        println!("│ │ ├─ {}", import);
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
}
