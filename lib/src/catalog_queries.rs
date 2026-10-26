//! Derived, graph-free catalog queries shared by live candidates and snapshots.
use crate::api::ImportPaths;
use crate::environment::Environment;
use crate::ontology::{GraphIdentifier, Ontology};
use anyhow::{anyhow, Result};
use oxigraph::model::NamedNode;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

#[derive(Debug)]
pub(crate) struct CatalogIndex {
    names: HashMap<String, GraphIdentifier>,
    imports: HashMap<GraphIdentifier, Vec<Result<GraphIdentifier, NamedNode>>>,
    importers: HashMap<GraphIdentifier, Vec<GraphIdentifier>>,
}

impl CatalogIndex {
    pub(crate) fn new(environment: &Environment) -> Self {
        let mut names = HashMap::new();
        // Match Environment's first canonical match in external catalogs that
        // can still contain competing registrations. Managed catalogs are unique.
        for id in environment.ontologies().keys() {
            names
                .entry(Environment::normalize_name(id.name().as_str()).to_owned())
                .or_insert_with(|| id.clone());
        }
        names.extend(
            environment
                .aliases()
                .iter()
                .filter(|(_, id)| environment.ontologies().contains_key(*id))
                .map(|(name, id)| (name.clone(), id.clone())),
        );
        let mut imports = HashMap::new();
        let mut importers: HashMap<_, Vec<_>> = HashMap::new();
        // Stable order for explanations and namespace precedence across handles.
        let mut ontologies: Vec<_> = environment.ontologies().values().collect();
        ontologies.sort_by(|a, b| a.name().as_str().cmp(b.name().as_str()));
        for ontology in ontologies {
            let mut resolved = Vec::new();
            let mut seen = HashSet::new();
            for name in &ontology.imports {
                if let Some(id) = names.get(Environment::normalize_name(name.as_str())) {
                    if seen.insert(id.clone()) {
                        resolved.push(Ok(id.clone()));
                        importers
                            .entry(id.clone())
                            .or_default()
                            .push(ontology.id().clone());
                    }
                } else {
                    resolved.push(Err(name.clone()));
                }
            }
            imports.insert(ontology.id().clone(), resolved);
        }
        Self {
            names,
            imports,
            importers,
        }
    }

    pub(crate) fn closure(
        &self,
        root: &GraphIdentifier,
        depth: i32,
        strict: bool,
    ) -> Result<Vec<GraphIdentifier>> {
        if !self.imports.contains_key(root) {
            return Err(anyhow!("Ontology {} not found", root.name()));
        }
        let mut visited = HashSet::from([root.clone()]);
        let mut result = vec![root.clone()];
        let mut queue = VecDeque::from([(root, 0)]);
        while let Some((id, level)) = queue.pop_front() {
            if depth >= 0 && level >= depth {
                continue;
            }
            for import in &self.imports[id] {
                match import {
                    Ok(id) if visited.insert(id.clone()) => {
                        result.push(id.clone());
                        queue.push_back((id, level + 1));
                    }
                    Err(name) if strict => return Err(anyhow!("Import not found: {name}")),
                    _ => {}
                }
            }
        }
        Ok(result)
    }

    pub(crate) fn importers(&self, name: &NamedNode) -> Result<Vec<GraphIdentifier>> {
        let id = self
            .names
            .get(Environment::normalize_name(name.as_str()))
            .ok_or_else(|| anyhow!("Ontology not found"))?;
        Ok(self.importers.get(id).cloned().unwrap_or_default())
    }

    pub(crate) fn explain_import(&self, name: &NamedNode) -> ImportPaths {
        if let Some(id) = self.names.get(Environment::normalize_name(name.as_str())) {
            return ImportPaths::Present(self.paths(id));
        }
        let mut importers: Vec<_> = self
            .imports
            .iter()
            .filter(|(_, imports)| {
                imports
                    .iter()
                    .any(|import| matches!(import, Err(missing) if missing == name))
            })
            .map(|(id, _)| id)
            .collect();
        importers.sort_by_key(|id| id.name().as_str());
        ImportPaths::Missing {
            importers: importers
                .into_iter()
                .flat_map(|id| self.paths(id))
                .collect(),
        }
    }

    fn paths(&self, target: &GraphIdentifier) -> Vec<Vec<GraphIdentifier>> {
        fn visit(
            index: &CatalogIndex,
            id: &GraphIdentifier,
            path: &mut Vec<GraphIdentifier>,
            result: &mut Vec<Vec<GraphIdentifier>>,
        ) {
            path.push(id.clone());
            let next: Vec<_> = index
                .importers
                .get(id)
                .into_iter()
                .flatten()
                .filter(|source| !path.contains(source))
                .collect();
            if next.is_empty() {
                result.push(path.iter().rev().cloned().collect());
            } else {
                for source in next {
                    visit(index, source, path, result);
                }
            }
            path.pop();
        }
        let mut result = Vec::new();
        visit(self, target, &mut Vec::new(), &mut result);
        result
    }
}

pub(crate) fn namespaces<'a>(
    ontologies: impl IntoIterator<Item = &'a Ontology>,
    bindings: &BTreeMap<String, String>,
) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for ontology in ontologies {
        result.extend(ontology.source_prefixes().clone());
        result.extend(ontology.namespace_map().clone());
    }
    let bound: HashSet<_> = bindings.values().collect();
    result.retain(|_, ns| !bound.contains(ns));
    result.extend(bindings.clone());
    result
}
