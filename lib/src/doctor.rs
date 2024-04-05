use crate::consts::*;
use crate::ontology::OntologyLocation;
use crate::OntoEnv;
use anyhow::Result;
use oxigraph::model::NamedNode;
use std::collections::HashMap;

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
        for ontology in env.ontologies.values() {
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
