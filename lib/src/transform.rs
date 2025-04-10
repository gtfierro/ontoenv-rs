//! Provides functions for transforming RDF graphs and datasets within the OntoEnv context.
//! This includes rewriting SHACL prefixes and removing OWL imports or ontology declarations.

use crate::consts::{DECLARE, IMPORTS, ONTOLOGY, PREFIXES, TYPE};
use oxigraph::model::{
    Dataset, Graph, NamedNodeRef, Quad, QuadRef, SubjectRef, TermRef, Triple, TripleRef,
};

/// Rewrites all sh:prefixes in the graph to point to the provided root
pub fn rewrite_sh_prefixes(graph: &mut Dataset, root: SubjectRef) {
    let mut to_remove: Vec<Quad> = vec![];
    let mut to_add: Vec<Quad> = vec![];
    // find all sh:prefixes quads
    for quad in graph.quads_for_predicate(PREFIXES) {
        let s = quad.subject;
        let g = quad.graph_name;
        let new_quad = QuadRef::new(s, PREFIXES, root, g);
        // remove the old quad <shape or rule, sh:prefixes, ontology>
        to_remove.push(quad.into());
        // add a new quad <shape or rule, sh:prefixes, root>
        to_add.push(new_quad.into());
    }
    // move the sh:declare statements to the root ontology too
    for quad in graph.quads_for_predicate(DECLARE) {
        let o = quad.object;
        let g = quad.graph_name;
        let new_quad = QuadRef::new(root, DECLARE, o, g);
        // remove the old quad <ontology, sh:declare, prefix>
        to_remove.push(quad.into());
        // add a new quad <root, sh:declare, prefix>
        to_add.push(new_quad.into());
    }

    // apply all changes
    for quad in to_remove {
        graph.remove(quad.as_ref());
    }
    for quad in to_add {
        graph.insert(quad.as_ref());
    }
}

pub fn rewrite_sh_prefixes_graph(graph: &mut Graph, root: SubjectRef) {
    let mut to_remove: Vec<Triple> = vec![];
    let mut to_add: Vec<Triple> = vec![];
    // find all sh:prefixes triples
    for triple in graph.triples_for_predicate(PREFIXES) {
        let s = triple.subject;
        let new_triple = TripleRef::new(s, PREFIXES, root);
        // remove the old triple <shape or rule, sh:prefixes, ontology>
        to_remove.push(triple.into());
        // add a new triple <shape or rule, sh:prefixes, root>
        to_add.push(new_triple.into());
    }
    // move the sh:declare statements to the root ontology too
    for triple in graph.triples_for_predicate(DECLARE) {
        let o = triple.object;
        let new_triple = TripleRef::new(root, DECLARE, o);
        // remove the old triple <ontology, sh:declare, prefix>
        to_remove.push(triple.into());
        // add a new triple <root, sh:declare, prefix>
        to_add.push(new_triple.into());
    }

    // apply all changes
    for triple in to_remove {
        graph.remove(triple.as_ref());
    }
    for triple in to_add {
        graph.insert(triple.as_ref());
    }
}

/// Remove owl:imports statements from a graph. Can be helpful to do after computing the union of
/// all imports so that downstream tools do not attempt to fetch these graph dependencies
/// themselves. If ontologies_to_remove is provided, only remove owl:imports to those ontologies
pub fn remove_owl_imports(graph: &mut Dataset, ontologies_to_remove: Option<&[NamedNodeRef]>) {
    let to_remove: Vec<Quad> = graph
        .quads_for_predicate(IMPORTS)
        .filter_map(|quad| match quad.object {
            TermRef::NamedNode(obj) => {
                if ontologies_to_remove.map_or(true, |ontologies| ontologies.contains(&obj)) {
                    Some(quad.into())
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    // Remove the collected quads
    for quad in to_remove {
        graph.remove(quad.as_ref());
    }
}

/// Remove owl:imports statements from a graph. Can be helpful to do after computing the union of
/// all imports so that downstream tools do not attempt to fetch these graph dependencies
/// themselves
pub fn remove_owl_imports_graph(graph: &mut Graph, ontologies_to_remove: Option<&[NamedNodeRef]>) {
    let to_remove: Vec<Triple> = graph
        .triples_for_predicate(IMPORTS)
        .filter_map(|triple| match triple.object {
            TermRef::NamedNode(obj) => {
                if ontologies_to_remove.map_or(true, |ontologies| ontologies.contains(&obj)) {
                    Some(triple.into())
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    // Remove the collected quads
    for quad in to_remove {
        graph.remove(quad.as_ref());
    }
}

/// Removes owl:Ontology declarations which are not the provided root
pub fn remove_ontology_declarations(graph: &mut Dataset, root: SubjectRef) {
    // remove owl:Ontology declarations that are not the first graph
    let mut to_remove: Vec<Quad> = vec![];
    for quad in graph.quads_for_object(ONTOLOGY) {
        let s = quad.subject;
        let p = quad.predicate;
        if p == TYPE && s != root {
            to_remove.push(quad.into());
        }
    }
    for quad in to_remove {
        graph.remove(quad.as_ref());
    }
}

/// Removes owl:Ontology declarations which are not the provided root
pub fn remove_ontology_declarations_graph(graph: &mut Graph, root: SubjectRef) {
    // remove owl:Ontology declarations that are not the first graph
    let mut to_remove: Vec<Triple> = vec![];
    for triple in graph.triples_for_object(ONTOLOGY) {
        let s = triple.subject;
        let p = triple.predicate;
        if p == TYPE && s != root {
            to_remove.push(triple.into());
        }
    }
    for triple in to_remove {
        graph.remove(triple.as_ref());
    }
}
