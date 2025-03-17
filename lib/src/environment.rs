use crate::io::GraphIO;
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use crate::policy;
use anyhow::Result;
use chrono::prelude::*;
use oxigraph::model::{
    Dataset, Graph, GraphName, NamedNode, NamedNodeRef, NamedOrBlankNode, QuadRef, Subject,
    SubjectRef,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A struct that holds the ontology environment: all the mappings
/// between ontology names and their respective graph identifiers and locations.
#[derive(Debug, Serialize, Deserialize)]
pub struct Environment {
    ontologies: HashMap<GraphIdentifier, Ontology>,
    #[serde(serialize_with = "policy::policy_serialize", deserialize_with = "policy::policy_deserialize")]
    default_policy: Box<dyn policy::ResolutionPolicy>,
    locations: HashMap<OntologyLocation, GraphIdentifier>,
}

impl Clone for Environment {
    fn clone(&self) -> Self {
        Self {
            ontologies: self.ontologies.clone(),
            locations: self.locations.clone(),
            default_policy: policy::policy_from_name(self.default_policy.policy_name())
                .expect("Failed to clone policy"),
        }
    }
}

impl Environment {
    pub fn new() -> Self {
        Self {
            ontologies: HashMap::new(),
            default_policy: Box::new(policy::DefaultPolicy),
            locations: HashMap::new(),
        }
    }

    pub fn ontologies(&self) -> &HashMap<GraphIdentifier, Ontology> {
        &self.ontologies
    }

    pub fn add_ontology(&mut self, mut ontology: Ontology) {
        ontology.last_updated = Some(Utc::now());
        self.locations
            .insert(ontology.location().unwrap().clone(), ontology.id().clone());
        self.ontologies.insert(ontology.id().clone(), ontology);
    }

    pub fn remove_ontology(&mut self, id: &GraphIdentifier) -> Option<Ontology> {
        self.locations
            .remove(self.ontologies.get(id)?.location().unwrap());
        self.ontologies.remove(id)
    }

    pub fn get_modified_time(&self, id: &GraphIdentifier) -> Option<DateTime<Utc>> {
        self.ontologies
            .get(id)
            .map(|ontology| ontology.last_updated)
            .flatten()
    }

    pub fn graphid_from_location(&self, location: &OntologyLocation) -> Option<&GraphIdentifier> {
        self.locations.get(location)
    }

    /// Returns an Ontology with the given id using the default policy
    pub fn get_ontology(&self, id: &GraphIdentifier) -> Option<Ontology> {
        self.get_ontology_with_policy(id.into(), &*self.default_policy)
    }

    /// Returns an Ontology with the given name. Uses the provided policy to resolve
    /// the ontology if there are multiple ontologies with the same name.
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

    /// Returns the first ontology with the given name
    pub fn get_ontology_by_name(&self, name: NamedNodeRef) -> Option<&Ontology> {
        // choose the first ontology with the given name
        self.ontologies
            .values()
            .find(|&ontology| ontology.name() == name)
    }

    /// Returns the first graph with the given name
    pub fn get_graph_by_name(&self, name: NamedNodeRef, store: impl GraphIO) -> Result<Graph> {
        let ontology = self
            .get_ontology_by_name(name)
            .ok_or(anyhow::anyhow!(format!("Ontology {} not found", name)))?;
        store.get_graph(ontology.id())
    }

    /// Returns the first ontology with the given location
    pub fn get_ontology_by_location(&self, location: &OntologyLocation) -> Option<&Ontology> {
        // choose the first ontology with the given location
        self.ontologies
            .values()
            .find(|&ontology| ontology.location() == Some(location))
    }
}
