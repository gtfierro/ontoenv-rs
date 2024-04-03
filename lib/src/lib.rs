extern crate derive_builder;

pub mod config;
pub mod consts;
pub mod ontology;
pub mod policy;
#[macro_use]
pub mod util;

use crate::config::Config;
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use anyhow::Result;
use chrono::prelude::*;
use log::{debug, error, info};
use oxigraph::model::{
    Dataset, Graph, GraphName, NamedNode, NamedNodeRef, NamedOrBlankNode, QuadRef,
};
use oxigraph::store::Store;
use petgraph::graph::{Graph as DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::{HashSet, VecDeque};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

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
    #[serde(skip, default = "default_store")]
    store: Store,
}

// probably need some graph "identifier" that incorporates location and version..

impl OntoEnv {
    pub fn new(config: Config) -> Result<Self> {
        // create the config.root/.ontoenv directory so it exists before the store
        // is created
        let ontoenv_dir = config.root.join(".ontoenv");
        std::fs::create_dir_all(&ontoenv_dir)?;

        // create the store in the root/.ontoenv/store.db directory
        let store = Store::open(&ontoenv_dir.join("store.db"))?;
        Ok(Self {
            config,
            ontologies: HashMap::new(),
            dependency_graph: DiGraph::new(),
            store,
        })
    }

    pub fn close(self) {}

    pub fn num_graphs(&self) -> usize {
        self.ontologies.len()
    }

    pub fn num_triples(&self) -> Result<usize> {
        // this construction coerces the error the the correct type
        Ok(self.store.len()?)
    }

    pub fn get_ontology_with_policy(&self, name: NamedNodeRef, policy: &dyn policy::ResolutionPolicy) -> Option<Ontology> {
        let ontologies = self.ontologies.values().collect::<Vec<&Ontology>>();
        policy.resolve(name.as_str(), &ontologies.as_slice()).map(|o| o.clone())
    }

    pub fn get_ontology_by_name(&self, name: NamedNodeRef) -> Option<&Ontology> {
        // choose the first ontology with the given name
        for ontology in self.ontologies.values() {
            if ontology.name() == name {
                return Some(ontology);
            }
        }
        None
    }

    fn get_ontology_by_location(&self, location: &OntologyLocation) -> Option<&Ontology> {
        // choose the first ontology with the given location
        for ontology in self.ontologies.values() {
            if ontology.location() == Some(location) {
                return Some(ontology);
            }
        }
        None
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let reader = BufReader::new(file);
        let env: OntoEnv = serde_json::from_reader(reader)?;
        // load store from root/.ontoenv/store.db
        let ontoenv_dir = env.config.root.join(".ontoenv");
        let store = Store::open(&ontoenv_dir.join("store.db"))?;
        Ok(Self { store, ..env })
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
        // save the dependency graph
        let dep_graph_path = ontoenv_dir.join("dependency_graph.json");
        let dep_graph_str = serde_json::to_string_pretty(&self.dependency_graph)?;
        let mut file = std::fs::File::create(dep_graph_path)?;
        file.write_all(dep_graph_str.as_bytes())?;
        Ok(())
    }

    fn update_dependency_graph(&mut self, updated_ids: Option<Vec<GraphIdentifier>>) -> Result<()> {
        // traverse the owl:imports closure and build the dependency graph
        let mut stack: VecDeque<GraphIdentifier> = match updated_ids {
            Some(ids) => ids.into(),
            None => self.ontologies.keys().cloned().collect(),
        };

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
                        error!("Failed to read ontology file: {}", e);
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
                // if location is a file and the file does not exist, remove the ontology
                if let OntologyLocation::File(path) = location {
                    if !path.exists() {
                        to_remove.push(ontology.clone());
                    }
                }
            }
        }
        for ontology in to_remove.iter() {
            info!("Removing ontology: {:?}", ontology);
            self.ontologies.remove(&ontology);
        }
        Ok(to_remove)
    }

    fn check_for_updates(&self) -> Result<Vec<GraphIdentifier>> {
        let mut updates = vec![];
        for (id, ontology) in self.ontologies.iter() {
            if let Some(location) = ontology.location() {
                if let OntologyLocation::File(f) = location {
                    let path = f.to_path_buf();
                    let metadata = std::fs::metadata(&path)?;
                    let last_updated: chrono::DateTime<Utc> = metadata.modified()?.into();
                    if last_updated > ontology.last_updated.unwrap() {
                        updates.push(id.clone());
                    }
                }
            }
        }
        Ok(updates)
    }

    fn get_environment_update_time(&self) -> Result<Option<chrono::DateTime<Utc>>> {
        let ontoenv_path = self.config.root.join(".ontoenv/ontoenv.json");
        if !ontoenv_path.exists() {
            return Ok(None);
        }
        let ontoenv_metadata = std::fs::metadata(&ontoenv_path)?;
        Ok(Some(ontoenv_metadata.modified()?.into()))
    }

    /// Load all graphs from the search directories
    pub fn update(&mut self) -> Result<()> {
        // get the date of modification for ontoenv.json
        let ontoenv_last_updated = self.get_environment_update_time()?;

        debug!("Building file cache");
        // make a cache of all files in the ontologies property
        let mut existing_files: HashSet<PathBuf> = HashSet::new();
        for ontology in self.ontologies.values() {
            if let Some(location) = ontology.location() {
                if let OntologyLocation::File(f) = location {
                    existing_files.insert(f.to_owned());
                }
            }
        }

        debug!("Finding new updates and files");
        // find all files in the search directories
        let files = self.find_files()?;
        for file in files {
            debug!("Reading file: {}", file.as_str());

            // if the file is in the cache and is older than the last update time, skip it
            if let OntologyLocation::File(f) = &file {
                if existing_files.contains(f) {
                    let metadata = std::fs::metadata(f)?;
                    let last_updated: chrono::DateTime<Utc> = metadata.modified()?.into();
                    if let Some(ontoenv_updated) = ontoenv_last_updated {
                        if last_updated < ontoenv_updated {
                            debug!("Skipping file: {}", f.display());
                            continue;
                        }
                    }
                }
            }

            // read the graph in the file and get a reference to the ontology record
            match self.add_or_update_ontology_from_location(file) {
                Ok(_) => continue,
                Err(e) => {
                    error!("Failed to read ontology file: {}", e);
                    continue;
                }
            };
        }

        // remove all ontologies that are no longer in the search directories
        self.remove_old_ontologies()?;

        info!("Checking for updates");
        let updated_ids = self.check_for_updates()?;
        if updated_ids.len() > 0 {
            info!("Updating ontologies: {:?}", updated_ids);
        }

        // update the dependency graph for the remaining ontologies
        info!("Updating dependency graphs for updated ontologies");
        self.update_dependency_graph(Some(updated_ids))?;

        // optimize the store for storage + queries
        info!("Optimizing store");
        self.store.optimize()?;

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
            let index = *indexes.entry(ontology.clone()).or_insert_with(|| {
                graph.add_node(ontology.name().into_owned())
            });
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
                let import_index = *indexes.entry(import.clone()).or_insert_with(|| {
                    graph.add_node(name)
                });
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
        let dot = petgraph::dot::Dot::with_config(&graph, &[]);
        Ok(format!("{:?}", dot))
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

        // if one is not found and the location is a URL then add the ontology to the environment
        let graph = match location.graph() {
            Ok(graph) => graph,
            Err(e) => {
                error!("Failed to read ontology {:?} location: {}", location, e);
                return Err(e);
            }
        };

        let mut ontology =
            Ontology::from_graph(&graph, location, self.config.require_ontology_names)?;
        ontology.with_last_updated(Utc::now());
        let id = ontology.id().clone();
        self.ontologies.insert(id.clone(), ontology);

        // if the graph is already in the store, remove it and add the new graph
        let graphname: NamedOrBlankNode = match id.graphname()? {
            GraphName::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
            _ => return Err(anyhow::anyhow!("Graph name not found")),
        };

        if self.store.contains_named_graph(graphname.as_ref())? {
            self.store.remove_named_graph(graphname.as_ref())?;
        }

        info!("Adding graph to store: {:?}", graphname);
        for triple in graph.into_iter() {
            let q: QuadRef = QuadRef::new(
                triple.subject,
                triple.predicate,
                triple.object,
                graphname.as_ref(),
            );
            self.store.insert(q)?;
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
        for quad in self
            .store
            .quads_for_pattern(None, None, None, Some(name.as_ref()))
        {
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
        Ok(closure.into_iter().collect())
    }

    /// Returns a graph containing the union of all graphs_ids
    pub fn get_union_graph(&self, graph_ids: &[GraphIdentifier]) -> Result<Dataset> {
        let mut union: Dataset = Dataset::new();
        for id in graph_ids {
            let graphname: NamedOrBlankNode = match id.graphname()? {
                GraphName::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
                _ => continue,
            };

            if !self.store.contains_named_graph(graphname.as_ref())? {
                return Err(anyhow::anyhow!("Graph not found: {:?}", id));
            }

            for quad in
                self.store
                    .quads_for_pattern(None, None, None, Some(id.graphname()?.as_ref()))
            {
                union.insert(quad?.as_ref());
            }

            //let d = g.into_dataset();
            //graph.insert_all(d.quads())?;
        }
        Ok(union)
    }

    pub fn dump(&self) {
        let mut ontologies = self.ontologies.clone();
        let mut groups: HashMap<NamedNode, Vec<Ontology>> = HashMap::new();
        for ontology in ontologies.values_mut() {
            let name = ontology.name();
            groups.entry(name).or_default().push(ontology.clone());
        }
        let mut sorted_groups: Vec<NamedNode> = groups.keys().cloned().collect();
        sorted_groups.sort_by(|a, b| a.cmp(b));
        for name in sorted_groups {
            let group = groups.get(&name).unwrap();
            println!("┌ Ontology: {}", name);
            for ontology in group {
                let g = self.get_graph(ontology.id()).unwrap();
                println!("├─ Location: {}", ontology.location().unwrap());
                // sorted keys
                let mut sorted_keys: Vec<NamedNode> =
                    ontology.version_properties().keys().cloned().collect();
                sorted_keys.sort_by(|a, b| a.cmp(b));
                // print up until last key
                if sorted_keys.len() > 0 {
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
                if ontology.imports.len() > 0 {
                    println!("│ ├─ Triples: {}", g.len());
                    println!("│ ├─ Imports:");
                    let mut sorted_imports: Vec<NamedNode> = ontology.imports.clone();
                    sorted_imports.sort_by(|a, b| a.cmp(b));
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
    use std::ffi::OsStr;
    use tempdir::TempDir;

    fn setup() -> TempDir {
        // create a temp directory and put all the tests/data files in it
        let dir = TempDir::new("ontoenv").unwrap();
        let data_dir = Path::new("tests/data");
        for entry in walkdir::WalkDir::new(data_dir) {
            let entry = entry.unwrap();
            let path = entry.path();
            let dest = dir.path().join(path);
            // create os str
            if path.is_file() && path.extension().unwrap_or(&OsStr::new("")) == "ttl" {
                println!("Copying {:?} to {:?}", path, dest);
                std::fs::copy(path, dest).unwrap();
            } else {
                // create directory if it doesn't exist
                std::fs::create_dir_all(dest).unwrap();
            }
        }
        dir
    }

    fn teardown(dir: TempDir) {
        dir.close().unwrap();
    }

    #[test]
    fn test_ontoenv_scans() {
        let dir = setup();
        let test_dir = dir.path().join("tests/data");
        // test that the ontoenv can scan the search directories and find all
        // the ontologies
        let cfg1 = Config::new(
            dir.path().into(),
            vec![test_dir.into()],
            &["*.ttl"],
            &[""],
            false,
        )
        .unwrap();
        let mut env = OntoEnv::new(cfg1).unwrap();
        env.update().unwrap();
        assert_eq!(env.num_graphs(), 18);
        teardown(dir);
    }

    #[test]
    fn test_ontoenv_update() {
        let dir = setup();
        let test_dir = dir.path().join("tests/data");
        // test that the ontoenv can update the environment
        let cfg1 = Config::new(
            dir.path().into(),
            vec![test_dir.into()],
            &["*.ttl"],
            &[""],
            false,
        )
        .unwrap();
        let mut env = OntoEnv::new(cfg1).unwrap();
        env.update().unwrap();
        assert_eq!(env.num_graphs(), 18);

        // delete tempdir's brickpatches.ttl file
        std::fs::remove_file(dir.path().join("tests/data/support/brickpatches.ttl")).unwrap();

        env.update().unwrap();
        assert_eq!(env.num_graphs(), 17);

        // copy brickpatches.ttl back
        let old_patches = Path::new("tests/data/support/brickpatches.ttl");
        std::fs::copy(
            old_patches,
            dir.path().join("tests/data/support/brickpatches.ttl"),
        )
        .unwrap();
        env.update().unwrap();
        assert_eq!(env.num_graphs(), 18);

        teardown(dir);
    }

    #[test]
    fn test_ontoenv_retrieval_by_name() {
        let dir = setup();
        let test_dir = dir.path().join("tests/data");
        let cfg1 = Config::new(
            dir.path().into(),
            vec![test_dir.into()],
            &["*.ttl"],
            &[""],
            false,
        )
        .unwrap();
        let mut env = OntoEnv::new(cfg1).unwrap();
        env.update().unwrap();

        let brick = NamedNodeRef::new("https://brickschema.org/schema/1.4-rc1/Brick").unwrap();
        let ont = env.get_ontology_by_name(brick).unwrap();
        assert_eq!(ont.imports.len(), 10);
        assert!(ont.location().unwrap().is_file());
    }

    #[test]
    fn test_ontoenv_retrieval_by_location() {
        let dir = setup();
        let test_dir = dir.path().join("tests/data");
        let cfg1 = Config::new(
            dir.path().into(),
            vec![test_dir.clone().into()],
            &["*.ttl"],
            &[""],
            false,
        )
        .unwrap();
        let mut env = OntoEnv::new(cfg1).unwrap();
        env.update().unwrap();

        let brick_path = test_dir.join("Brick-1.4-rc1.ttl");
        let loc = OntologyLocation::from_str(brick_path.to_str().unwrap()).unwrap();
        let ont = env.get_ontology_by_location(&loc).unwrap();
        assert_eq!(ont.imports.len(), 10);
        assert!(ont.location().unwrap().is_file());
    }

    #[test]
    fn test_ontoenv_load() {
        let dir = setup();
        let test_dir = dir.path().join("tests/data");
        let cfg1 = Config::new(
            dir.path().into(),
            vec![test_dir.into()],
            &["*.ttl"],
            &[""],
            false,
        )
        .unwrap();
        let mut env = OntoEnv::new(cfg1).unwrap();
        env.update().unwrap();
        assert_eq!(env.num_graphs(), 18);
        env.save_to_directory().unwrap();
        // drop env
        env.close();

        // reload env
        let cfg_location = dir.path().join(".ontoenv").join("ontoenv.json");
        println!("Loading from: {:?}", cfg_location);
        let env2 = OntoEnv::from_file(cfg_location.as_path())
            .expect(format!("Failed to load from {:?}", cfg_location).as_str());
        assert_eq!(env2.num_graphs(), 18);
    }
}
