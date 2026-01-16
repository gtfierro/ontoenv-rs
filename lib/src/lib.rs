//! `ontoenv` is an environment manager for ontologies. It can be used as a Rust library to manage local and remote RDF ontologies and their dependencies.
//!
//! It recursively discovers and resolves `owl:imports` statements, and provides an API for querying the dependency graph and retrieving a unified "imports closure" of an ontology.
//!
//! The environment is backed by an `Oxigraph` store.
//!
//! # Usage
//!
//! Here is a basic example of how to use the `ontoenv` Rust library. This example will:
//! 1. Create a temporary directory.
//! 2. Write two simple ontologies to files in that directory, where one imports the other.
//! 3. Configure and initialize `ontoenv` to use this directory.
//! 4. Compute the dependency closure of one ontology to demonstrate that `ontoenv` correctly resolves and includes the imported ontology.
//!
//! ```rust
//! use ontoenv::config::Config;
//! use ontoenv::ToUriString;
//! use ontoenv::api::{OntoEnv, ResolveTarget};
//! use oxigraph::model::NamedNode;
//! use std::path::PathBuf;
//! use std::fs;
//! use std::io::Write;
//! use std::collections::HashSet;
//!
//! # fn main() -> anyhow::Result<()> {
//! // Set up a temporary directory for the example
//! let test_dir = PathBuf::from("target/doc_test_temp_readme");
//! if test_dir.exists() {
//!     fs::remove_dir_all(&test_dir)?;
//! }
//! fs::create_dir_all(&test_dir)?;
//! let root = test_dir.canonicalize()?;
//!
//! // Create a dummy ontology file for ontology A
//! let ontology_a_path = root.join("ontology_a.ttl");
//! let mut file_a = fs::File::create(&ontology_a_path)?;
//! writeln!(file_a, r#"
//! @prefix owl: <http://www.w3.org/2002/07/owl#> .
//! @prefix : <http://example.com/ontology_a#> .
//! <http://example.com/ontology_a> a owl:Ontology .
//! "#)?;
//!
//! // Create a dummy ontology file for ontology B which imports A
//! let ontology_b_path = root.join("ontology_b.ttl");
//! let mut file_b = fs::File::create(&ontology_b_path)?;
//! writeln!(file_b, r#"
//! @prefix owl: <http://www.w3.org/2002/07/owl#> .
//! @prefix : <http://example.com/ontology_b#> .
//! <http://example.com/ontology_b> a owl:Ontology ;
//!     owl:imports <http://example.com/ontology_a> .
//! "#)?;
//!
//! // Configure ontoenv
//! let config = Config::builder()
//!     .root(root.clone())
//!     .locations(vec![root.clone()])
//!     .temporary(true) // Use a temporary environment
//!     .build()?;
//!
//! // Initialize the environment
//! let mut env = OntoEnv::init(config, false)?;
//! env.update()?;
//!
//! // Check that our ontologies were loaded
//! let ontologies = env.ontologies();
//! assert_eq!(ontologies.len(), 2);
//!
//! // Get the dependency closure for ontology B
//! let ont_b_name = NamedNode::new("http://example.com/ontology_b")?;
//! let ont_b_id = env.resolve(ResolveTarget::Graph(ont_b_name)).unwrap();
//! let closure_ids = env.get_closure(&ont_b_id, -1)?;
//!
//! // The closure should contain both ontology A and B
//! assert_eq!(closure_ids.len(), 2);
//! let closure_names: HashSet<String> = closure_ids.iter().map(|id| id.to_uri_string()).collect();
//! assert!(closure_names.contains("http://example.com/ontology_a"));
//! assert!(closure_names.contains("http://example.com/ontology_b"));
//!
//! // We can also get the union graph of the closure
//! let union_graph_result = env.get_union_graph(&closure_ids, Some(false), Some(false))?;
//! // Each ontology has 1 triple, so the union should have 2.
//! // the 'ontology_a' declaration gets removed by default so that the closure
//! // only has one ontology declaration.
//! assert_eq!(union_graph_result.dataset.len(), 2);
//!
//! // Clean up
//! fs::remove_dir_all(&test_dir)?;
//! # Ok(())
//! # }
//! ```

extern crate derive_builder;

pub mod api;
pub mod config;
pub mod consts;
pub mod doctor;
pub mod environment;
pub mod errors;
pub mod fetch;
pub mod io;
pub mod ontology;
pub mod options;
pub mod policy;
#[macro_use]
pub mod util;
pub mod transform;

use crate::ontology::GraphIdentifier;
use chrono::prelude::*;
use oxigraph::model::NamedNode;
use pretty_bytes::converter::convert as pretty_bytes;
use std::fmt::{self, Display};
use std::path::{Path, PathBuf};

// Small helper trait to normalize identifiers into URI strings across the
// library without leaking concrete graph/node types into public APIs.
pub trait ToUriString {
    fn to_uri_string(&self) -> String;
}

impl ToUriString for NamedNode {
    fn to_uri_string(&self) -> String {
        self.as_str().to_string()
    }
}

// Accept borrowed nodes to reduce string allocation at call sites.
impl ToUriString for &NamedNode {
    fn to_uri_string(&self) -> String {
        self.as_str().to_string()
    }
}

// GraphIdentifier is an internal wrapper used by the environment; implementing
// ToUriString keeps diagnostics and formatting consistent with NamedNode.
impl ToUriString for GraphIdentifier {
    fn to_uri_string(&self) -> String {
        self.name().as_str().to_string()
    }
}

// Mirror the owned impl so callers can pass &GraphIdentifier ergonomically.
impl ToUriString for &GraphIdentifier {
    fn to_uri_string(&self) -> String {
        self.name().as_str().to_string()
    }
}

// Keep error context lightweight and printable without tying to a specific
// error type; this gets surfaced in CLI output and logs.
pub struct FailedImport {
    ontology: GraphIdentifier,
    error: String,
}

impl FailedImport {
    pub fn new(ontology: GraphIdentifier, error: String) -> Self {
        // Store only the minimum context needed for user-facing diagnostics.
        Self { ontology, error }
    }
}

impl Display for FailedImport {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Failed to import ontology {}: {}",
            self.ontology.to_uri_string(),
            self.error
        )
    }
}

// Snapshot of the current on-disk environment for `doctor`-style diagnostics.
// We keep it minimal and copyable to avoid holding locks on the store.
pub struct EnvironmentStatus {
    // true if there is an environment that ontoenv can find
    exists: bool,
    // absolute path to the .ontoenv directory
    ontoenv_path: Option<PathBuf>,
    // number of ontologies in the environment
    num_ontologies: usize,
    // last time the environment was updated
    last_updated: Option<DateTime<Utc>>,
    // size of the oxigraph store, in bytes
    store_size: u64,
    // list of missing imports
    missing_imports: Vec<NamedNode>,
}

impl EnvironmentStatus {
    pub fn exists(&self) -> bool {
        // Fast check for callers that only care whether an env was found.
        self.exists
    }

    pub fn ontoenv_path(&self) -> Option<&Path> {
        // Expose path as a borrow to avoid cloning.
        self.ontoenv_path.as_deref()
    }

    pub fn num_ontologies(&self) -> usize {
        // Report the current catalog size for status and UX.
        self.num_ontologies
    }

    pub fn last_updated(&self) -> Option<&DateTime<Utc>> {
        // Provide a reference to preserve the original timestamp precision.
        self.last_updated.as_ref()
    }

    pub fn store_size(&self) -> u64 {
        // Keep bytes for programmatic use; formatting happens in Display.
        self.store_size
    }

    pub fn missing_imports(&self) -> &[NamedNode] {
        // Return a slice so callers cannot mutate internal state.
        &self.missing_imports
    }
}

// impl Display pretty print for EnvironmentStatus
impl std::fmt::Display for EnvironmentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if !self.exists {
            // Avoid printing stale metadata when the environment is missing.
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
        let ontoenv_path = self
            .ontoenv_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "N/A".to_string());
        // Pretty-print store size so humans can spot growth/regressions quickly.
        write!(
            f,
            "Environment Path: {}\n\
            Number of Ontologies: {}\n\
            Last Updated: {}\n\
            Store Size: {}",
            ontoenv_path,
            self.num_ontologies,
            last_updated,
            pretty_bytes(self.store_size as f64),
        )?;

        if !self.missing_imports.is_empty() {
            write!(f, "\n\nMissing Imports:")?;
            for import in &self.missing_imports {
                // Render as URI for consistency with other environment output.
                write!(f, "\n  - {}", import.to_uri_string())?;
            }
        }
        Ok(())
    }
}
