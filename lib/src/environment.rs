//! Defines the `Environment` struct which holds the core state of the ontology environment,
//! including the collection of ontologies, their locations, and the default resolution policy.

use crate::io::GraphIO;
use crate::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use crate::policy;
use anyhow::{anyhow, Result};
use chrono::prelude::*;
use log::warn;
use oxigraph::model::{Graph, NamedNode, NamedNodeRef};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// Represents the loaded ontology environment, including ontologies, their source
/// locations, normalized aliases, and the default resolution policy.
#[derive(Debug, Serialize, Deserialize)]
pub struct Environment {
    #[serde(serialize_with = "ontologies_ser", deserialize_with = "ontologies_de")]
    ontologies: HashMap<GraphIdentifier, Ontology>,
    #[serde(
        serialize_with = "policy::policy_serialize",
        deserialize_with = "policy::policy_deserialize"
    )]
    default_policy: Box<dyn policy::ResolutionPolicy>,
    #[serde(skip)]
    pub locations: HashMap<OntologyLocation, GraphIdentifier>,
    #[serde(default)]
    aliases: HashMap<String, GraphIdentifier>,
}

impl Clone for Environment {
    fn clone(&self) -> Self {
        Self {
            ontologies: self.ontologies.clone(),
            locations: self.locations.clone(),
            aliases: self.aliases.clone(),
            default_policy: policy::policy_from_name(self.default_policy.policy_name())
                .expect("Failed to clone policy"),
        }
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment {
    fn normalize_name(s: &str) -> &str {
        let trimmed_hash = s.trim_end_matches('#');
        trimmed_hash.trim_end_matches('/')
    }

    pub fn new() -> Self {
        Self {
            ontologies: HashMap::new(),
            default_policy: Box::new(policy::DefaultPolicy),
            locations: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    pub fn ontologies(&self) -> &HashMap<GraphIdentifier, Ontology> {
        &self.ontologies
    }

    pub fn add_ontology(&mut self, mut ontology: Ontology) -> Result<()> {
        ontology.last_updated = Some(Utc::now());
        let location = ontology
            .location()
            .cloned()
            .ok_or_else(|| anyhow!("Cannot add ontology {} without a location", ontology.id()))?;
        let ontology_id = ontology.id().clone();
        let ontology_name = ontology.name();
        self.locations.insert(location.clone(), ontology_id.clone());
        self.register_alias(&location, &ontology_id, &ontology_name);
        self.ontologies.insert(ontology_id, ontology);
        Ok(())
    }

    pub fn remove_ontology(&mut self, id: &GraphIdentifier) -> Result<Option<Ontology>> {
        if let Some(existing) = self.ontologies.get(id) {
            if let Some(location) = existing.location() {
                self.locations.remove(location);
            } else {
                warn!("Removing ontology {} without recorded location", id);
            }
            self.aliases.retain(|_, value| value != id);
        }
        Ok(self.ontologies.remove(id))
    }

    pub fn get_modified_time(&self, id: &GraphIdentifier) -> Option<DateTime<Utc>> {
        self.ontologies
            .get(id)
            .and_then(|ontology| ontology.last_updated)
    }

    pub fn graphid_from_location(&self, location: &OntologyLocation) -> Option<&GraphIdentifier> {
        self.locations.get(location)
    }

    /// Returns a cloned `Ontology` for the provided identifier using the default resolution policy.
    pub fn get_ontology(&self, id: &GraphIdentifier) -> Option<Ontology> {
        self.get_ontology_with_policy(id.into(), &*self.default_policy)
    }

    /// Returns a cloned `Ontology` with the given name, resolving conflicts with the supplied policy.
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

    /// Returns the first ontology whose name (or registered alias) matches the supplied value.
    pub fn get_ontology_by_name(&self, name: NamedNodeRef) -> Option<&Ontology> {
        let target = Self::normalize_name(name.as_str());
        if let Some(id) = self.aliases.get(target) {
            if let Some(ontology) = self.ontologies.get(id) {
                return Some(ontology);
            }
        }
        self.ontologies.values().find(|ontology| {
            let binding = ontology.name();
            let candidate = Self::normalize_name(binding.as_str());
            candidate == target
        })
    }

    /// Returns the graph associated with the given name (respecting aliases) using the provided I/O backend.
    pub fn get_graph_by_name(&self, name: NamedNodeRef, store: impl GraphIO) -> Result<Graph> {
        let ontology = self
            .get_ontology_by_name(name)
            .ok_or(anyhow::anyhow!(format!("Ontology {} not found", name)))?;
        store.get_graph(ontology.id())
    }

    /// Returns the first ontology with the given location
    pub fn get_ontology_by_location(&self, location: &OntologyLocation) -> Option<&Ontology> {
        let id = self.locations.get(location)?;
        self.ontologies.get(id)
    }

    fn register_alias(
        &mut self,
        location: &OntologyLocation,
        ontology_id: &GraphIdentifier,
        ontology_name: &NamedNode,
    ) {
        if let OntologyLocation::Url(url) = location {
            if let Ok(loc_node) = NamedNode::new(url.clone()) {
                let loc_norm = Self::normalize_name(loc_node.as_str()).to_string();
                let name_norm = Self::normalize_name(ontology_name.as_str());
                if loc_norm != name_norm {
                    self.aliases.insert(loc_norm, ontology_id.clone());
                } else {
                    self.aliases.remove(&loc_norm);
                }
            }
        }
    }

    pub fn rebuild_aliases(&mut self) {
        self.aliases.clear();
        let mut alias_data: Vec<(OntologyLocation, GraphIdentifier, NamedNode)> = Vec::new();
        for ontology in self.ontologies.values() {
            if let Some(location) = ontology.location() {
                alias_data.push((location.clone(), ontology.id().clone(), ontology.name()));
            }
        }
        for (location, ontology_id, ontology_name) in alias_data {
            self.register_alias(&location, &ontology_id, &ontology_name);
        }
    }
}
