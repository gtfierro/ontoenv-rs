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


#[derive(Serialize, Deserialize, Hash)]
pub enum OntologyLocation {
    File(PathBuf),
    Url(String),
}

impl OntologyLocation {
    pub fn as_str(&self) -> &str {
        match self {
            OntologyLocation::File(p) => p.to_str().unwrap_or_default(),
            OntologyLocation::Url(u) => u.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Result<Self> {
        if s.starts_with("http") {
            Ok(OntologyLocation::Url(s.to_string()))
        } else {
            // make sure the path starts with file://
            let path = if s.starts_with("file://") {
                s.to_string()
            } else {
                format!("file://{}", s)
            };
            Ok(OntologyLocation::File(PathBuf::from(path)))
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Ontology {
    name: Iri<String>,
    pub imports: Vec<Iri<String>>,
    pub location: Option<OntologyLocation>,
    pub last_updated: Option<time::SystemTime>,
    // optional version string
    version: Option<String>,
}

impl Ontology {
    pub fn new(name: Iri<String>, imports: &[[&SimpleTerm; 3]]) -> Result<Self> {
        let imports_iri: Vec<Iri<String>> = imports
            .iter()
            .map(|t| match t[2] {
                SimpleTerm::Iri(iri) => {
                    Iri::new(iri.as_str().to_string()).map_err(|e| anyhow::anyhow!(e))
                }
                _ => Err(anyhow::anyhow!("Import is not an IRI")),
            })
            .collect::<Result<Vec<Iri<String>>>>()?;

        Ok(Ontology {
            name,
            imports: imports_iri,
            location: None,
            version: None,
            last_updated: time::SystemTime::now().into(),
        })
    }

    pub fn with_location(mut self, location: OntologyLocation) -> Self {
        self.location = Some(location);
        self
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl From<LightGraph> for Ontology {
    fn from(graph: LightGraph) -> Self {
        // get the rdf:type owl:Ontology declarations
        let decls = graph
            .triples_matching(Any, [TYPE], [ONTOLOGY])
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // ontology_name is the subject of the first declaration
        let ontology_name = match decls.first() {
            Some(decl) => match decl.s() {
                SimpleTerm::Iri(iri) => Iri::new(iri.as_str().to_string()).unwrap(),
                _ => panic!("Ontology name is not an IRI"),
            },
            None => panic!("No ontology declaration found"),
        };
        let imports = graph
            .triples_matching([ontology_name.clone()], [IMPORTS], Any)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // we can check the version of the ontology in several ways:

        Ontology {
            name: ontology_name,
            imports: imports
                .iter()
                .map(|t| match t[2] {
                    SimpleTerm::Iri(iri) => Iri::new(iri.as_str().to_string()).unwrap(),
                    _ => panic!("Import is not an IRI"),
                })
                .collect(),
            location: None,
            version: None,
            last_updated: time::SystemTime::now().into(),
        }
    }
}

