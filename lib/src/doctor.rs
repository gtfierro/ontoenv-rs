//! Provides functionality for diagnosing potential issues within an OntoEnv environment.
//! Defines traits and structs for environment checks and reporting problems.

use crate::api::OntoEnv;
use crate::consts::*;
use crate::ontology::OntologyLocation;
use anyhow::Result;
use oxigraph::model::NamedNode;
use std::collections::{BTreeSet, HashMap, HashSet};

pub struct OntologyProblem {
    pub locations: Vec<OntologyLocation>,
    pub message: String,
}

pub trait EnvironmentCheck {
    fn name(&self) -> &str;
    fn check(&mut self, env: &OntoEnv, problems: &mut Vec<OntologyProblem>) -> Result<()>;
}

pub struct Doctor {
    checks: Vec<Box<dyn EnvironmentCheck>>,
}

impl Default for Doctor {
    fn default() -> Self {
        Self::new()
    }
}

impl Doctor {
    pub fn new() -> Self {
        Self { checks: Vec::new() }
    }

    pub fn add_check(&mut self, check: Box<dyn EnvironmentCheck>) {
        self.checks.push(check);
    }

    pub fn run(&mut self, env: &OntoEnv) -> Result<Vec<OntologyProblem>> {
        let mut problems = Vec::new();
        for check in &mut self.checks {
            check.check(env, &mut problems)?;
        }
        Ok(problems)
    }
}

pub struct OntologyDeclaration {}

impl EnvironmentCheck for OntologyDeclaration {
    fn name(&self) -> &str {
        "Ontology Declaration"
    }

    fn check(&mut self, env: &OntoEnv, problems: &mut Vec<OntologyProblem>) -> Result<()> {
        for location in env.find_files()? {
            let g = match location.graph() {
                Ok(g) => g,
                Err(e) => {
                    problems.push(OntologyProblem {
                        locations: vec![location.clone()],
                        message: format!("Failed to load graph: {}", e),
                    });
                    continue;
                }
            };

            let decls: Vec<_> = g
                .subjects_for_predicate_object(TYPE, ONTOLOGY)
                .collect::<Vec<_>>();
            if decls.is_empty() {
                problems.push(OntologyProblem {
                    locations: vec![location.clone()],
                    message: "No ontology declaration found".to_string(),
                });
            } else if decls.len() > 1 {
                problems.push(OntologyProblem {
                    locations: vec![location.clone()],
                    message: "Multiple ontology declarations found".to_string(),
                });
            }
        }
        Ok(())
    }
}

pub struct DuplicateOntology {}

impl EnvironmentCheck for DuplicateOntology {
    fn name(&self) -> &str {
        "Duplicate Ontology"
    }

    fn check(&mut self, env: &OntoEnv, problems: &mut Vec<OntologyProblem>) -> Result<()> {
        // group ontologies by name; if there are more than one in a group, report an error
        let mut names: HashMap<NamedNode, Vec<OntologyLocation>> = HashMap::new();
        for ontology in env.ontologies().values() {
            let name = ontology.name();
            names
                .entry(name)
                .or_default()
                .push(ontology.location().unwrap().clone());
        }
        for (name, locations) in names {
            if locations.len() > 1 {
                problems.push(OntologyProblem {
                    locations,
                    message: format!("Multiple ontologies with name {}", name),
                });
            }
        }

        Ok(())
    }
}

pub struct ConflictingPrefixes {}

impl EnvironmentCheck for ConflictingPrefixes {
    fn name(&self) -> &str {
        "Conflicting Prefixes"
    }

    fn check(&mut self, env: &OntoEnv, problems: &mut Vec<OntologyProblem>) -> Result<()> {
        let mut reported_conflicts: HashSet<(String, BTreeSet<String>)> = HashSet::new();

        for root_ontology in env.ontologies().values() {
            let closure_ids = env.get_dependency_closure(root_ontology.id())?;

            // prefix -> { namespace -> [locations] }
            let mut closure_prefix_map: HashMap<String, HashMap<String, Vec<OntologyLocation>>> =
                HashMap::new();

            for graph_id in &closure_ids {
                let ontology = env.get_ontology(graph_id)?;
                let ns_map = ontology.namespace_map();
                if let Some(location) = ontology.location() {
                    for (prefix, namespace) in ns_map {
                        closure_prefix_map
                            .entry(prefix.to_string())
                            .or_default()
                            .entry(namespace.to_string())
                            .or_default()
                            .push(location.clone());
                    }
                }
            }

            for (prefix, ns_mappings) in closure_prefix_map {
                if ns_mappings.len() > 1 {
                    // This prefix has conflicting namespaces within this closure.
                    let conflicting_namespaces: BTreeSet<String> =
                        ns_mappings.keys().cloned().collect();
                    let conflict_key = (prefix.clone(), conflicting_namespaces);

                    if reported_conflicts.insert(conflict_key) {
                        // This is a new conflict we haven't reported yet.
                        let all_locations = ns_mappings.into_values().flatten().collect();
                        problems.push(OntologyProblem {
                            locations: all_locations,
                            message: format!(
                                "Conflicting namespace definitions for prefix '{}'",
                                prefix
                            ),
                        });
                    }
                }
            }
        }

        Ok(())
    }
}
