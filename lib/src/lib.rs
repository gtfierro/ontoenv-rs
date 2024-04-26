extern crate derive_builder;

pub mod config;
pub mod consts;
pub mod doctor;
pub mod errors;
pub mod ontology;
pub mod policy;
#[macro_use]
pub mod util;
pub mod transform;

use crate::config::Config;
use crate::consts::{IMPORTS, ONTOLOGY, PREFIXES, TYPE};
use crate::doctor::{Doctor, DuplicateOntology, OntologyDeclaration};
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use anyhow::Result;
use chrono::prelude::*;
use log::{debug, error, info, warn};
use oxigraph::model::{
    Dataset, Graph, GraphName, NamedNode, NamedNodeRef, NamedOrBlankNode, Quad, QuadRef, SubjectRef,
};
use oxigraph::store::Store;
use petgraph::graph::{Graph as DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::{HashSet, VecDeque};
use std::io::{BufReader, Write};
use std::path::Path;

// custom derive for ontologies field as vec of Ontology
fn ontologies_ser<S>(
    ontologies: &HashMap<GraphIdentifier, Ontology>,
    s: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let vec: Vec<&Ontology> = ontologies.values().collect();
    vec.serialize(s)
}

fn ontologies_de<'de, D>(d: D) -> Result<HashMap<GraphIdentifier, Ontology>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let vec: Vec<Ontology> = Vec::deserialize(d)?;
    let mut map = HashMap::new();
    for ontology in vec {
        map.insert(ontology.id().clone(), ontology);
    }
    Ok(map)
}

fn default_store() -> Store {
    Store::new().unwrap()
}

#[derive(Serialize, Deserialize)]
pub struct OntoEnv {
    config: Config,
    #[serde(serialize_with = "ontologies_ser", deserialize_with = "ontologies_de")]
    ontologies: HashMap<GraphIdentifier, Ontology>,
    dependency_graph: DiGraph<GraphIdentifier, (), petgraph::Directed>,
    #[serde(skip)]
    read_only: bool,
}

// probably need some graph "identifier" that incorporates location and version..

impl OntoEnv {
    pub fn new(config: Config) -> Result<Self> {
        // create the config.root/.ontoenv directory so it exists before the store
        // is created
        let ontoenv_dir = config.root.join(".ontoenv");
        std::fs::create_dir_all(&ontoenv_dir)?;

        // create the store in the root/.ontoenv/store.db directory
        Ok(Self {
            config,
            ontologies: HashMap::new(),
            dependency_graph: DiGraph::new(),
            read_only: false,
        })
    }

    // TODO: add a read-only version? make this thread-safe?
    fn store(&self) -> Result<Store> {
        let ontoenv_dir = self.config.root.join(".ontoenv");
        std::fs::create_dir_all(&ontoenv_dir)?;
        Store::open(ontoenv_dir.join("store.db"))
            .map_err(|e| anyhow::anyhow!("Could not open store: {}", e))
    }

    pub fn new_readonly(config: Config) -> Result<Self> {
        // create the store in the root/.ontoenv/store.db directory
        let store = Store::open_secondary(config.root.join(".ontoenv/store.db"))?;
        Ok(Self {
            config,
            ontologies: HashMap::new(),
            dependency_graph: DiGraph::new(),
            read_only: true,
        })
    }

    pub fn close(self) {}

    //TODO: add import_graph which imports a single graph into a given graph

    pub fn num_graphs(&self) -> usize {
        self.ontologies.len()
    }

    pub fn num_triples(&self) -> Result<usize> {
        // this construction coerces the error the the correct type
        Ok(self.store()?.len()?)
    }

    pub fn get_ontology_with_policy(
        &self,
        name: NamedNodeRef,
        policy: &dyn policy::ResolutionPolicy,
    ) -> Option<Ontology> {
        let ontologies = self.ontologies.values().collect::<Vec<&Ontology>>();
        policy
            .resolve(name.as_str(), ontologies.as_slice())
            .cloned()
    }

    pub fn get_ontology_by_name(&self, name: NamedNodeRef) -> Option<&Ontology> {
        // choose the first ontology with the given name
        self.ontologies
            .values()
            .find(|&ontology| ontology.name() == name)
    }

    pub fn get_graph_by_name(&self, name: NamedNodeRef) -> Result<Graph> {
        let ontology = self
            .get_ontology_by_name(name)
            .ok_or(anyhow::anyhow!("Ontology not found"))?;
        self.get_graph(ontology.id())
    }

    fn get_ontology_by_location(&self, location: &OntologyLocation) -> Option<&Ontology> {
        // choose the first ontology with the given location
        self.ontologies
            .values()
            .find(|&ontology| ontology.location() == Some(location))
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let reader = BufReader::new(file);
        let env: OntoEnv = serde_json::from_reader(reader)?;
        Ok(Self {
            read_only: false,
            ..env
        })
    }

    /// creates a new directory called .ontoenv in self.root and saves:
    /// - the configuration as ontoenv.json
    /// - all graphs in the environment as .ttl files
    /// - the dependency graph as a json file
    pub fn save_to_directory(&self) -> Result<()> {
        let ontoenv_dir = self.config.root.join(".ontoenv");
        info!("Saving ontology environment to: {:?}", ontoenv_dir);
        std::fs::create_dir_all(&ontoenv_dir)?;
        // save the configuration
        let config_path = ontoenv_dir.join("ontoenv.json");
        let config_str = serde_json::to_string_pretty(&self)?;
        let mut file = std::fs::File::create(config_path)?;
        file.write_all(config_str.as_bytes())?;
        Ok(())
    }

    fn update_dependency_graph(&mut self, updated_ids: Option<Vec<GraphIdentifier>>) -> Result<()> {
        // traverse the owl:imports closure and build the dependency graph
        let mut stack: VecDeque<GraphIdentifier> = match updated_ids {
            Some(ids) => ids.into(),
            None => self.ontologies.keys().cloned().collect(),
        };

        info!("Using # updated ids: {:?}", stack.len());

        while let Some(ontology) = stack.pop_front() {
            info!("Building dependency graph for: {:?}", ontology);
            let ont = self
                .ontologies
                .get(&ontology)
                .ok_or(anyhow::anyhow!("Ontology not found"))?;
            let imports = &ont.imports.clone();
            for import in imports {
                if let Some(_imp) = self.get_ontology_by_name(import.into()) {
                    continue;
                }
                info!("Adding import: {}", import);
                let location = OntologyLocation::from_str(import.as_str())?;
                let imp = match self.add_or_update_ontology_from_location(location) {
                    Ok(imp) => imp,
                    Err(e) => {
                        error!("Failed to read ontology file {}: {}", import.as_str(), e);
                        continue;
                    }
                };
                stack.push_back(imp);
            }
        }

        // put the dependency graph into self.dependency_graph
        let mut indexes: HashMap<GraphIdentifier, NodeIndex> = HashMap::new();
        let mut graph: DiGraph<GraphIdentifier, (), petgraph::Directed> = DiGraph::new();
        // add all ontologies in self.ontologies to the graph
        for ontology in self.ontologies.keys() {
            let index = graph.add_node(ontology.clone());
            indexes.insert(ontology.clone(), index);
        }
        // traverse the ontologies and add edges to the graph
        for ontology in self.ontologies.keys() {
            let index = indexes.get(ontology).unwrap();
            let ont = match self.ontologies.get(ontology) {
                Some(ont) => ont,
                None => {
                    error!("Ontology not found: {:?}", ontology);
                    continue;
                }
            };
            for import in &ont.imports {
                let graph_id = match self.get_ontology_by_name(import.into()) {
                    Some(imp) => imp.id(),
                    None => {
                        error!("Import not found: {}", import);
                        continue;
                    }
                };
                let import_index = indexes.get(graph_id).unwrap();
                graph.add_edge(*index, *import_index, ());
            }
        }
        // update the dependency graph
        self.dependency_graph = graph;
        Ok(())
    }

    /// Remove all ontologies that are no longer in the search directories
    /// and return a list of the removed ontologies
    fn remove_old_ontologies(&mut self) -> Result<Vec<GraphIdentifier>> {
        // check for any ontologies that are no longer in the search directories
        let mut to_remove: Vec<GraphIdentifier> = vec![];
        for ontology in self.ontologies.keys() {
            let location = self
                .ontologies
                .get(ontology)
                .ok_or(anyhow::anyhow!("Ontology not found"))?
                .location();
            if let Some(location) = location {
                // if location is a file and the file does not exist or it is no longer in the set
                // of included paths, remove the ontology
                if let OntologyLocation::File(path) = location {
                    if !path.exists() || !self.config.is_included(path) {
                        to_remove.push(ontology.clone());
                    }
                }
            }
        }
        for ontology in to_remove.iter() {
            debug!("Removing ontology: {:?}", ontology);
            self.ontologies.remove(ontology);
        }
        Ok(to_remove)
    }

    /// Returns a list of all files in the internal index that have been updated
    fn get_updated_indexed_files(&self) -> Result<Vec<GraphIdentifier>> {
        let mut updates = vec![];
        for (id, ontology) in self.ontologies.iter() {
            if let Some(location) = ontology.location() {
                if let OntologyLocation::File(f) = location {
                    let path = f.to_path_buf();
                    let metadata = std::fs::metadata(&path)?;

                    let last_updated: chrono::DateTime<Utc> = metadata.modified()?.into();

                    info!(
                        "Ontology: {:?}, last updated: {:?}; current: {:?}",
                        id, ontology.last_updated, last_updated
                    );
                    if last_updated >= ontology.last_updated.unwrap() {
                        updates.push(id.clone());
                    }
                }
            }
        }
        //
        Ok(updates)
    }

    /// Returns a list of all files in the environment which have been updated (added or changed)
    /// Does not return files that have been removed
    pub fn get_updated_files(&self) -> Result<Vec<OntologyLocation>> {
        // make a cache of all files in the ontologies property
        let mut existing_files: HashSet<OntologyLocation> = HashSet::new();
        for ontology in self.ontologies.values() {
            if let Some(location) = ontology.location() {
                if let OntologyLocation::File(_) = location {
                    existing_files.insert(location.clone());
                }
            }
        }
        let new_files: HashSet<OntologyLocation> = self
            .find_files()?
            .into_iter()
            .filter(|file| !existing_files.contains(file))
            .collect();
        let updated_ids = self.get_updated_indexed_files()?;
        if !updated_ids.is_empty() {
            info!("Updating ontologies: {:?}", updated_ids);
        }
        let mut updated_files: HashSet<OntologyLocation> = updated_ids
            .iter()
            .filter_map(|id| {
                self.ontologies
                    .get(id)
                    .and_then(|ont| ont.location().cloned())
            })
            .collect::<HashSet<OntologyLocation>>();

        // compute the union of new_files and updated_files
        updated_files.extend(new_files);
        Ok(updated_files.into_iter().collect())
    }

    fn get_environment_update_time(&self) -> Result<Option<chrono::DateTime<Utc>>> {
        let ontoenv_path = self.config.root.join(".ontoenv/ontoenv.json");
        if !ontoenv_path.exists() {
            return Ok(None);
        }
        let ontoenv_metadata = std::fs::metadata(&ontoenv_path)?;
        Ok(Some(ontoenv_metadata.modified()?.into()))
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
        // Step one: remove all ontologies that are no longer in the search directories
        self.remove_old_ontologies()?;

        info!("Checking for updates");
        // Step two: find all new and updated files
        let updated_files = self.get_updated_files()?;

        // Step three: add or update the ontologies from the new and updated files

        let updated_ids: Vec<GraphIdentifier> = if self.config.strict {
            let updated_ids: Result<Vec<GraphIdentifier>> = updated_files
                .into_iter()
                .map(|file| self.add_or_update_ontology_from_location(file.clone()))
                .collect();
            // handle error reporting
            updated_ids.map_err(|e| {
                error!("Failed to read ontology file: {}", e);
                e
            })?
        } else {
            updated_files
                .into_iter()
                .map(|file| self.add_or_update_ontology_from_location(file.clone()))
                .filter_map(|r| r.ok())
                .collect()
        };

        // Step four: update the dependency graph for all updated ontologies
        info!("Updating dependency graphs for updated ontologies");
        self.update_dependency_graph(Some(updated_ids))?;

        // optimize the store for storage + queries
        info!("Optimizing store");
        self.store()?.optimize()?;

        Ok(())
    }

    /// Returns the GraphViz dot representation of the dependency graph
    pub fn dep_graph_to_dot(&self) -> Result<String> {
        self.rooted_dep_graph_to_dot(self.ontologies.keys().cloned().collect())
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
                .ontologies
                .get(&ontology)
                .ok_or(anyhow::anyhow!("Ontology not found"))?;
            for import in &ont.imports {
                let import = match self.get_ontology_by_name(import.into()) {
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

    fn find_files(&self) -> Result<Vec<OntologyLocation>> {
        let mut files = vec![];
        for search_directory in &self.config.search_directories {
            for entry in walkdir::WalkDir::new(search_directory) {
                let entry = entry?;
                if entry.file_type().is_file() && self.config.is_included(entry.path()) {
                    files.push(OntologyLocation::File(entry.path().to_path_buf()));
                }
            }
        }
        Ok(files)
    }

    pub fn add(&mut self, location: OntologyLocation) -> Result<GraphIdentifier> {
        self.add_or_update_ontology_from_location(location)
    }

    /// Add or update the ontology from the given location. Overwrites the ontology
    /// if it already exists in the environment.
    fn add_or_update_ontology_from_location(
        &mut self,
        location: OntologyLocation,
    ) -> Result<GraphIdentifier> {
        //// find an entry in self.ontologies with the same Location
        //if let Some(ontology) = self.get_ontology_by_location(&location) {
        //    info!("Found ontology with the same location: {:?}", ontology);
        //    return Ok(ontology.id().clone());
        //}

        //// if one is not found, find a ontology with the same name
        //if let Some(ontology) = self.get_ontology_by_name(location.to_iri().as_ref()) {
        //    info!("Found ontology with the same name: {:?}", ontology);
        //    return Ok(ontology.id().clone());
        //}

        // if location is a Url and we are in offline mode, skip adding the ontology
        // and raise a warning
        if location.is_url() && self.config.offline {
            warn!("Offline mode is enabled, skipping URL: {:?}", location);
            return Err(anyhow::anyhow!("Offline mode is enabled"));
        }

        // if one is not found and the location is a URL then add the ontology to the environment
        let graph = match location.graph() {
            Ok(graph) => graph,
            Err(e) => {
                error!("Failed to read ontology {:?}: {}", location, e);
                return Err(e);
            }
        };

        let mut ontology =
            Ontology::from_graph(&graph, location, self.config.require_ontology_names)?;
        ontology.with_last_updated(Utc::now());
        info!(
            "Adding ontology: {:?} updated: {:?}",
            ontology.id(),
            ontology.last_updated
        );
        let id = ontology.id().clone();
        self.ontologies.insert(id.clone(), ontology);

        // if the graph is already in the store, remove it and add the new graph
        let graphname: NamedOrBlankNode = match id.graphname()? {
            GraphName::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
            _ => return Err(anyhow::anyhow!("Graph name not found")),
        };

        let store = self.store()?;

        if store.contains_named_graph(graphname.as_ref())? {
            store.remove_named_graph(graphname.as_ref())?;
        }

        info!("Adding graph to store: {:?}", graphname);
        for triple in graph.into_iter() {
            let q: QuadRef = QuadRef::new(
                triple.subject,
                triple.predicate,
                triple.object,
                graphname.as_ref(),
            );
            store.insert(q)?;
        }

        Ok(id)
    }

    pub fn graph_ids(&self) -> Vec<GraphIdentifier> {
        self.ontologies.keys().cloned().collect()
    }

    pub fn ontologies(&self) -> &HashMap<GraphIdentifier, Ontology> {
        &self.ontologies
    }

    /// returns a list of all graphs in the environment that provide a definition
    /// for the given IRI (using owl:Ontology)
    pub fn get_graphs_by_name(&self, name: NamedNodeRef) -> Vec<GraphIdentifier> {
        let mut graphs = vec![];
        for ontology in self.ontologies.values() {
            if ontology.name() == name {
                graphs.push(ontology.id().clone());
            }
        }
        graphs
    }

    pub fn get_graph(&self, id: &GraphIdentifier) -> Result<Graph> {
        let mut graph = Graph::new();
        let name = id.graphname()?;
        let store = self.store()?;
        for quad in store.quads_for_pattern(None, None, None, Some(name.as_ref())) {
            graph.insert(quad?.as_ref());
        }
        Ok(graph)
    }

    /// Returns a table of metadata for the given graph
    pub fn graph_metadata(&self, id: &GraphIdentifier) -> HashMap<String, String> {
        let mut metadata = HashMap::new();
        if let Some(ontology) = self.ontologies.get(id) {
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

    /// Returns the names of all graphs within the dependency closure of the provided graph
    pub fn get_dependency_closure(&self, id: &GraphIdentifier) -> Result<Vec<GraphIdentifier>> {
        let mut closure: HashSet<GraphIdentifier> = HashSet::new();
        let mut stack: VecDeque<GraphIdentifier> = VecDeque::new();
        stack.push_back(id.clone());
        while let Some(graph) = stack.pop_front() {
            closure.insert(graph.clone());
            let ontology = self
                .ontologies
                .get(&graph)
                .ok_or(anyhow::anyhow!("Ontology not found"))?;
            for import in &ontology.imports {
                // get graph identifier for import
                let import = match self.get_ontology_by_name(import.into()) {
                    Some(imp) => imp.id().clone(),
                    None => {
                        error!("Import not found: {}", import);
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

    /// Returns a graph containing the union of all graphs_ids
    pub fn get_union_graph(
        &self,
        graph_ids: &[GraphIdentifier],
        rewrite_sh_prefixes: Option<bool>,
        remove_owl_imports: Option<bool>,
    ) -> Result<Dataset> {
        // compute union of all graphs
        let mut union: Dataset = Dataset::new();
        let store = self.store()?;
        for id in graph_ids {
            let graphname: NamedOrBlankNode = match id.graphname()? {
                GraphName::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
                _ => continue,
            };

            if !store.contains_named_graph(graphname.as_ref())? {
                return Err(anyhow::anyhow!("Graph not found: {:?}", id));
            }

            let mut count = 0;
            for quad in store.quads_for_pattern(None, None, None, Some(id.graphname()?.as_ref())) {
                count += 1;
                union.insert(quad?.as_ref());
            }
            info!("Added {} triples from graph: {:?}", count, id);
        }
        let first_id = graph_ids
            .first()
            .ok_or(anyhow::anyhow!("No graphs found"))?;
        let root_ontology: SubjectRef = SubjectRef::NamedNode(first_id.name());

        // Rewrite sh:prefixes
        // defaults to true if not specified
        if let Some(true) = rewrite_sh_prefixes.or(Some(true)) {
            transform::rewrite_sh_prefixes(&mut union, root_ontology);
        }
        // remove owl:imports
        if let Some(true) = remove_owl_imports.or(Some(true)) {
            transform::remove_owl_imports(&mut union)
        }
        transform::remove_ontology_declarations(&mut union, root_ontology);
        Ok(union)
    }

    /// Returns a list of issues with the environment
    pub fn doctor(&self) {
        let mut doctor = Doctor::new();
        doctor.add_check(Box::new(DuplicateOntology {}));
        doctor.add_check(Box::new(OntologyDeclaration {}));

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

    pub fn dump(&self) {
        let mut ontologies = self.ontologies.clone();
        let mut groups: HashMap<NamedNode, Vec<Ontology>> = HashMap::new();
        for ontology in ontologies.values_mut() {
            let name = ontology.name();
            groups.entry(name).or_default().push(ontology.clone());
        }
        let mut sorted_groups: Vec<NamedNode> = groups.keys().cloned().collect();
        sorted_groups.sort();
        for name in sorted_groups {
            let group = groups.get(&name).unwrap();
            println!("┌ Ontology: {}", name);
            for ontology in group {
                let g = self.get_graph(ontology.id()).unwrap();
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

#[cfg(test)]
mod tests {

    use super::*;
    use std::path::PathBuf;
    use tempdir::TempDir;

    fn copy_file(path: &PathBuf, dest: &PathBuf) -> Result<()> {
        println!("Copying {:?} to {:?}", path, dest);
        if path.is_file() {
            std::fs::copy(path, dest)?;
        } else {
            std::fs::create_dir_all(dest)?;
        }
        Ok(())
    }

    fn setup(dir: &str) -> Result<TempDir> {
        // copy all files from tests/ to a temp directory and return the temp directory
        let test_dir = TempDir::new("ontoenv")?;
        // where test files are located
        let base_dir = Path::new("tests/").join(dir);
        println!("Copying files from {:?} to {:?}", base_dir, test_dir.path());
        // destination directory
        for entry in walkdir::WalkDir::new(&base_dir) {
            let entry = entry?;
            let path = entry.path();
            let dest = test_dir.path().join(path.strip_prefix(&base_dir)?);
            copy_file(&path.into(), &dest)?;
        }
        Ok(test_dir)
    }

    fn default_config(dir: &TempDir) -> Config {
        Config::new(
            dir.path().into(),
            Some(vec![dir.path().into()]),
            &["*.ttl"],
            &[""],
            false,
            false,
            true,
            "default".to_string(),
        )
        .unwrap()
    }

    fn default_config_with_subdir(dir: &TempDir, path: &str) -> Config {
        Config::new(
            dir.path().into(),
            Some(vec![dir.path().join(path)]),
            &["*.ttl"],
            &[""],
            false,
            false,
            true,
            "default".to_string(),
        )
        .unwrap()
    }

    // we don't care about errors when cleaning up the TempDir so
    // we just drop the TempDir (looking at this doc:
    // https://docs.rs/tempdir/latest/tempdir/struct.TempDir.html#method.close)
    fn teardown(_dir: TempDir) {}

    #[test]
    fn test_ontoenv_scans() -> Result<()> {
        let dir = setup("data2")?;
        let cfg = default_config(&dir);
        let mut env = OntoEnv::new(cfg)?;
        env.update()?;
        assert_eq!(env.num_graphs(), 4);
        teardown(dir);
        Ok(())
    }

    #[test]
    fn test_ontoenv_scans_default() -> Result<()> {
        let dir = setup("data2")?;
        let cfg = Config::new_with_default_matches(
            dir.path().into(),
            Some([dir.path().into()]),
            false,
            false,
            true,
        )?;
        let mut env = OntoEnv::new(cfg)?;
        env.update()?;
        assert_eq!(env.num_graphs(), 4);
        teardown(dir);
        Ok(())
    }

    #[test]
    fn test_ontoenv_num_triples() -> Result<()> {
        let dir = setup("fileendings")?;
        let cfg1 = Config::new(
            dir.path().into(),
            Some(vec![dir.path().into()]),
            &["*.n3"],
            &[""],
            false,
            false,
            true,
            "default".to_string(),
        )?;
        let mut env = OntoEnv::new(cfg1)?;
        env.update()?;
        assert_eq!(env.num_graphs(), 1);
        assert_eq!(env.num_triples()?, 5);
        teardown(dir);
        Ok(())
    }

    #[test]
    fn test_ontoenv_update() -> Result<()> {
        let dir = setup("data2")?;
        let cfg = default_config(&dir);
        let mut env = OntoEnv::new(cfg)?;
        env.update()?;
        let old_num_triples = env.num_triples()?;
        assert_eq!(env.num_graphs(), 4);

        // updating again shouldn't add anything
        env.update()?;
        assert_eq!(env.num_graphs(), 4);
        assert_eq!(env.num_triples()?, old_num_triples);

        // remove file
        std::fs::remove_file(dir.path().join("ont4.ttl"))?;

        env.update()?;
        assert_eq!(env.num_graphs(), 3);

        // copy ont4.ttl back
        let base_dir = Path::new("tests/").join("data2");
        std::fs::copy(base_dir.join("ont4.ttl"), dir.path().join("ont4.ttl"))?;
        env.update()?;
        assert_eq!(env.num_graphs(), 4);

        teardown(dir);
        Ok(())
    }

    #[test]
    fn test_ontoenv_retrieval_by_name() -> Result<()> {
        let dir = setup("data2")?;
        let cfg = default_config(&dir);
        let mut env = OntoEnv::new(cfg)?;
        env.update()?;

        let ont1 = NamedNodeRef::new("urn:ont1")?;
        let ont = env
            .get_ontology_by_name(ont1)
            .ok_or(anyhow::anyhow!("Ontology not found"))?;
        assert_eq!(ont.imports.len(), 2);
        assert!(ont.location().unwrap().is_file());
        teardown(dir);
        Ok(())
    }

    #[test]
    fn test_ontoenv_retrieval_by_location() -> Result<()> {
        let dir = setup("data2")?;
        let cfg = default_config(&dir);
        let mut env = OntoEnv::new(cfg)?;
        env.update()?;

        let ont1_path = dir.path().join("ont1.ttl");
        let loc = OntologyLocation::from_str(
            ont1_path
                .to_str()
                .ok_or(anyhow::anyhow!("Failed to convert to string"))?,
        )?;
        let ont = env
            .get_ontology_by_location(&loc)
            .ok_or(anyhow::anyhow!("Ontology not found"))?;
        assert_eq!(ont.imports.len(), 2);
        assert!(ont
            .location()
            .ok_or(anyhow::anyhow!("Location not found"))?
            .is_file());
        teardown(dir);
        Ok(())
    }

    #[test]
    fn test_ontoenv_load() -> Result<()> {
        let dir = setup("data2")?;
        let cfg = default_config(&dir);
        let mut env = OntoEnv::new(cfg)?;
        env.update()?;
        assert_eq!(env.num_graphs(), 4);
        env.save_to_directory()?;
        // drop env
        env.close();

        // reload env
        let cfg_location = dir.path().join(".ontoenv").join("ontoenv.json");
        let env2 = OntoEnv::from_file(cfg_location.as_path())?;
        assert_eq!(env2.num_graphs(), 4);
        teardown(dir);
        Ok(())
    }

    #[test]
    fn test_ontoenv_add() -> Result<()> {
        let dir = setup("updates")?;
        let cfg1 = default_config_with_subdir(&dir, "v1");
        let mut env = OntoEnv::new(cfg1)?;
        env.update()?;
        assert_eq!(env.num_graphs(), 4);

        let ont_path = dir.path().join("v2/ont5.ttl");
        let loc = OntologyLocation::from_str(
            ont_path
                .to_str()
                .ok_or(anyhow::anyhow!("Failed to convert to string"))?,
        )?;
        env.add(loc)?;
        assert_eq!(env.num_graphs(), 5);
        teardown(dir);
        Ok(())
    }

    #[test]
    fn test_ontoenv_detect_updates() -> Result<()> {
        let dir = setup("updates")?;
        let cfg1 = default_config_with_subdir(&dir, "v1");
        let mut env = OntoEnv::new(cfg1)?;
        env.update()?;
        assert_eq!(env.num_graphs(), 4);

        // copy files from dir/v2 to dir/v1
        let base_dir = Path::new("tests/").join("updates").join("v2");
        for entry in walkdir::WalkDir::new(&base_dir) {
            let entry = entry?;
            let path = entry.path();
            let dest = dir.path().join("v1").join(path.strip_prefix(&base_dir)?);
            println!("Copying {:?} without {:?} to {:?}", path, base_dir, dest);
            copy_file(&path.to_path_buf(), &dest)?;
        }

        env.update()?;

        assert_eq!(env.num_graphs(), 5);
        teardown(dir);
        Ok(())
    }

    #[test]
    fn test_check_for_updates() -> Result<()> {
        let dir = setup("updates")?;
        let cfg1 = default_config_with_subdir(&dir, "v1");
        let mut env = OntoEnv::new(cfg1)?;
        env.update()?;
        assert_eq!(env.num_graphs(), 4);

        // copy files from dir/v2 to dir/v1
        let base_dir = Path::new("tests/").join("updates").join("v2");
        for entry in walkdir::WalkDir::new(&base_dir) {
            let entry = entry?;
            let path = entry.path();
            let dest = dir.path().join("v1").join(path.strip_prefix(&base_dir)?);
            println!("Copying {:?} without {:?} to {:?}", path, base_dir, dest);
            copy_file(&path.to_path_buf(), &dest)?;
        }

        let updates = env.get_updated_files()?;
        assert_eq!(updates.len(), 1);
        teardown(dir);
        Ok(())
    }
}
