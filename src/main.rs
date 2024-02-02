mod ontology;

use anyhow::Result;
use std::time;
use env_logger;
use glob_match::glob_match;
use log::{error, info, warn};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use sophia::api::prelude::*;
use sophia::api::term::SimpleTerm;
use sophia::inmem::graph::LightGraph;
use sophia::iri::Iri;
use sophia::turtle::parser::turtle;
use sophia::xml::parser as xml;
use sophia_jsonld::parser as json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::env::current_dir;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use walkdir;
use crate::ontology::{Ontology, OntologyLocation};

const ONTOLOGY: Iri<&str> = Iri::new_unchecked_const("http://www.w3.org/2002/07/owl#Ontology");
const IMPORTS: Iri<&str> = Iri::new_unchecked_const("http://www.w3.org/2002/07/owl#imports");
const TYPE: Iri<&str> = Iri::new_unchecked_const("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");


// OntoEnv struct
#[derive(Serialize, Deserialize)]
pub struct OntoEnv {
    root: PathBuf,
    search_directories: Vec<PathBuf>,
    // include regex patterns
    includes: Option<Vec<String>>,
    // exclude patterns
    excludes: Option<Vec<String>>,
    // ontologies
    ontologies: Vec<Ontology>,
    // require ontology names?
    require_ontology_names: bool,
}

impl OntoEnv {
    // create new OntoEnv; take something that can be converted to a Path
    pub fn new<P: AsRef<Path>>(search: Vec<P>) -> Self {
        OntoEnv {
            root: current_dir().unwrap(),
            search_directories: search
                .into_iter()
                .map(|p| p.as_ref().to_path_buf())
                .collect(),
            includes: None::<Vec<String>>,
            excludes: None::<Vec<String>>,
            ontologies: Vec::new(),
            require_ontology_names: false,
        }
    }

    pub fn save_to_file(&self, file: &Path) -> Result<()> {
        let json = serde_json::to_string(self)?;
        std::fs::write(file, json)?;
        Ok(())
    }

    // set includes
    pub fn includes(&mut self, includes: Vec<String>) -> &mut OntoEnv {
        // convert each pattern to a regex
        self.includes = Some(includes);
        self
    }

    // set excludes
    pub fn excludes(&mut self, excludes: Vec<String>) -> &mut OntoEnv {
        // convert each pattern to a regex
        self.excludes = Some(excludes);
        self
    }

    fn get_ontology(&self, name: &str) -> Option<&Ontology> {
        self.ontologies.iter().find(|o| o.name() == name)
    }

    fn has_ontology(&self, name: &str) -> bool {
        self.get_ontology(name).is_some()
    }

    fn build_index(&mut self) -> Result<()> {
        info!("Building ontology index");
        // find all files in the search directories
        for file in self.find_files()? {
            info!("Reading ontology file: {}", file.display());
            match self.read_ontology_file(&file) {
                Ok(name) => {
                    self.ontologies.push(name);
                }
                Err(e) => {
                    error!("Failed to read ontology file: {} ({})", file.display(), e);
                    continue;
                }
            }
        }
        // for each ontology entry in self.ontologies:
        // - if the ontology name exists in self.ontology_map, then skip it
        // - otherwise, treat the ontology name as a url and fetch the ontology
        //   with read_ontology_url
        let mut names_to_visit: VecDeque<String> =
            self.ontologies.iter().map(|k| k.name().to_string()).collect();
        let mut visited: HashSet<String> = HashSet::new();
        while let Some(ont) = names_to_visit.pop_front() {
            if visited.contains(ont.as_str()) {
                continue;
            }
            visited.insert(ont.to_string());

            // fetch the ontology if it doesn't exist in the ontology_map
            if !self.has_ontology(ont.as_str()) {
                info!("Reading ontology url: {}", ont);
                if let Ok(ontology) = self.read_ontology_url(ont.as_str()) {
                    self.ontologies.push(ontology);
                } else {
                    error!("Failed to read ontology url: {}", ont);
                    continue;
                }
            }

            // get the ontology entry and add all of its imports to the queue
            if let Some(ont) = self.get_ontology(ont.as_str()) {
                for import in &ont.imports {
                    names_to_visit.push_back(import.as_str().to_string());
                }
            }
        }
        Ok(())
    }

    fn is_included(&self, path: &Path) -> bool {
        // check excludes
        if let Some(ref excludes) = self.excludes {
            for exclude in excludes {
                if glob_match(exclude, path.to_str().unwrap_or("")) {
                    return false;
                }
            }
        }
        if let Some(ref includes) = self.includes {
            for include in includes {
                if glob_match(include, path.to_str().unwrap_or("")) {
                    return true;
                }
            }
            return false;
        }
        true
    }

    fn find_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = vec![];
        for search_directory in &self.search_directories {
            for entry in walkdir::WalkDir::new(search_directory) {
                let entry = entry?;
                if entry.file_type().is_file() && self.is_included(entry.path()) {
                    files.push(entry.path().to_path_buf());
                }
            }
        }
        Ok(files)
    }

    fn extract_ontology_definition(
        &mut self,
        graph: &LightGraph,
        default_name: &str,
    ) -> Result<Ontology> {
        // get the rdf:type owl:Ontology declarations
        let decls = graph
            .triples_matching(Any, [TYPE], [ONTOLOGY])
            .collect::<Result<Vec<_>, _>>()?;

        // ontology_name is the subject of the first declaration
        let ontology_name = match decls.first() {
            Some(decl) => match decl.s() {
                SimpleTerm::Iri(iri) => Iri::new(iri.as_str().to_string())?,
                _ => return Err(anyhow::anyhow!("Ontology name is not an IRI")),
            },
            None => {
                if self.require_ontology_names {
                    return Err(anyhow::anyhow!(
                        "No ontology declaration found in {}",
                        default_name
                    ));
                }
                warn!("No ontology declaration found in {}", default_name);
                Iri::new(default_name.to_string())?
            }
        };
        let imports = graph
            .triples_matching([&ontology_name], [IMPORTS], Any)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Ontology::new(ontology_name.clone(), imports.as_slice())?)
    }

    fn read_ontology_url(&mut self, url: &str) -> Result<Ontology> {
        let client = reqwest::blocking::Client::new();
        let resp = client.get(url).header(CONTENT_TYPE, "text/turtle").send()?;
        if !resp.status().is_success() {
            return Err(anyhow::anyhow!("Failed to fetch ontology from {}", url));
        }
        // get the Content-Type header
        let content_type = resp.headers().get("Content-Type");
        let content: BufReader<_> = BufReader::new(reqwest::blocking::get(url)?);

        // determine the parser to use based on the Content-Type header
        let graph: LightGraph = match content_type {
            Some(content_type) => {
                if content_type.to_str()?.contains("xml") {
                    xml::parse_bufread(content).collect_triples()?
                } else if content_type.to_str()?.contains("turtle") {
                    turtle::parse_bufread(content).collect_triples()?
                } else if content_type.to_str()?.contains("ttl") {
                    turtle::parse_bufread(content).collect_triples()?
                } else {
                    return Err(anyhow::anyhow!(
                        "Unknown Content-Type: {}",
                        content_type.to_str()?
                    ));
                }
            }
            None => {
                let content = String::from_utf8(content.bytes().collect::<Result<Vec<u8>, _>>()?)?;
                // try to parse as ttl, then rdfxml, then fail
                match turtle::parse_str(&content).collect_triples() {
                    Ok(graph) => graph,
                    Err(_) => match xml::parse_str(&content).collect_triples() {
                        Ok(graph) => graph,
                        Err(_) => return Err(anyhow::anyhow!("Unknown Content-Type")),
                    },
                }
            }
        };

        // get the rdf:type owl:Ontology declarations
        Ok(self.extract_ontology_definition(&graph, url)?.with_location(OntologyLocation::Url(url.to_string())))
    }

    fn read_ontology_file(&mut self, filename: &Path) -> Result<Ontology> {
        let file = std::fs::File::open(filename)?;
        let content: BufReader<_> = BufReader::new(file);
        let graph: LightGraph = turtle::parse_bufread(content).collect_triples()?;
        // get the rdf:type owl:Ontology declarations
        // get ontolgoy location, making sure it starts with file://
        let location = format!("file://{}", filename.to_str().unwrap_or_default());
        Ok(self.extract_ontology_definition(
            &graph,
            location.as_str(),
        )?.with_location(OntologyLocation::File(PathBuf::from(location))))
    }

    pub fn dump_ontologies(&self) {
        for ontology in &self.ontologies {
            println!("Ontology: {}", ontology.name());
            if let Some(location) = &ontology.location {
                println!("  Location: {}", location.as_str());
            }
            // last accessed
            if let Some(last_updated) = &ontology.last_updated {
                // pretty timestamp
                let datetime: chrono::DateTime<chrono::Local> = last_updated.clone().into();
                println!("  Last Updated: {}", datetime.format("%Y-%m-%d %H:%M:%S"));
            }
            for import in &ontology.imports {
                println!("  Import: {}", import);
            }
        }
    }
}

fn main() -> Result<()> {
    env_logger::init();
    // read path from command line
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("Usage: {} <path>", args[0]);
        std::process::exit(1);
    }
    let paths = &args[1..];

    let mut onto = OntoEnv::new(paths.to_vec());
    onto.includes(vec![
        "*/**/*.ttl".to_string(),
        "*/**/*.xml".to_string(),
        "*/**/*.n3".to_string(),
    ]);
    onto.build_index()?;
    onto.save_to_file(Path::new("onto.json"))?;
    onto.dump_ontologies();
    Ok(())
}
