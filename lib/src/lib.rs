extern crate derive_builder;

pub mod api;
pub mod config;
pub mod consts;
pub mod doctor;
pub mod environment;
pub mod errors;
pub mod io;
pub mod ontology;
pub mod policy;
#[macro_use]
pub mod util;
pub mod transform;

use crate::config::{Config, HowCreated};
use crate::consts::{ONTOLOGY, TYPE};
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use anyhow::Result;
use chrono::prelude::*;
use log::{debug, error, info, warn};
use oxigraph::model::{
    Dataset, Graph, GraphName, NamedNode, NamedNodeRef, NamedOrBlankNode, QuadRef, Subject,
    SubjectRef,
};
use oxigraph::store::Store;
use petgraph::graph::{Graph as DiGraph, NodeIndex};
use pretty_bytes::converter::convert as pretty_bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::{HashSet, VecDeque};
use std::fmt::{self, Display};
use std::fs;
use std::io::{BufReader, Write};
use std::path::Path;
use walkdir::WalkDir;

pub struct FailedImport {
    ontology: GraphIdentifier,
    error: String,
}

impl FailedImport {
    pub fn new(ontology: GraphIdentifier, error: String) -> Self {
        Self { ontology, error }
    }
}

impl Display for FailedImport {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Failed to import ontology {}: {}",
            self.ontology, self.error
        )
    }
}

pub struct EnvironmentStatus {
    // true if there is an environment that ontoenv can find
    exists: bool,
    // number of ontologies in the environment
    num_ontologies: usize,
    // last time the environment was updated
    last_updated: Option<DateTime<Utc>>,
    // size of the oxigraph store, in bytes
    store_size: u64,
}

// impl Display pretty print for EnvironmentStatus
impl std::fmt::Display for EnvironmentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if !self.exists {
            return write!(f, "No environment found");
        }
        // convert last_updated to local timestamp, or display N/A if
        // it is None
        let last_updated = match self.last_updated {
            Some(last_updated) => last_updated
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S %Z")
                .to_string(),
            None => "N/A".to_string(),
        };
        write!(
            f,
            "Environment Status\n\
            Number of Ontologies: {}\n\
            Last Updated: {}\n\
            Store Size: {} bytes",
            self.num_ontologies,
            last_updated,
            pretty_bytes(self.store_size as f64),
        )
    }
}
