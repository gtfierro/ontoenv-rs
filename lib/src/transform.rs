//! Provides functions for transforming RDF graphs and datasets within the OntoEnv context.
//! This includes rewriting SHACL prefixes and removing OWL imports or ontology declarations.

use crate::consts::{DECLARE, IMPORTS, ONTOLOGY, PREFIXES, TYPE};
use oxigraph::model::{
    Dataset, Graph, NamedNodeRef, NamedOrBlankNodeRef, Quad, QuadRef, TermRef, Triple, TripleRef,
};
use std::collections::HashSet;

/// Rewrites all sh:prefixes in a graph to point to the provided root
pub fn rewrite_sh_prefixes_graph(graph: &mut Graph, root: NamedOrBlankNodeRef) {
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
    // move the sh:declare statements to the root ontology too, deduplicating by (sh:prefix, sh:namespace)
    let sh_prefix = NamedNodeRef::new_unchecked("http://www.w3.org/ns/shacl#prefix");
    let sh_namespace = NamedNodeRef::new_unchecked("http://www.w3.org/ns/shacl#namespace");
    let mut seen: HashSet<(String, String)> = HashSet::new();

    // Seed with any existing declarations on the root
    for t in graph.triples_for_predicate(DECLARE) {
        if t.subject == root {
            // Attempt to extract (prefix, namespace) pair
            if let Some(decl_node) = match t.object {
                TermRef::NamedNode(nn) => Some(NamedOrBlankNodeRef::NamedNode(nn)),
                TermRef::BlankNode(bn) => Some(NamedOrBlankNodeRef::BlankNode(bn)),
                _ => None,
            } {
                let mut pref: Option<String> = None;
                let mut ns: Option<String> = None;
                for t2 in graph.triples_for_subject(decl_node) {
                    if t2.predicate == sh_prefix {
                        if let TermRef::Literal(l) = t2.object {
                            pref = Some(l.value().to_string());
                        }
                    } else if t2.predicate == sh_namespace {
                        match t2.object {
                            TermRef::NamedNode(nn) => ns = Some(nn.as_str().to_string()),
                            TermRef::Literal(l) => ns = Some(l.value().to_string()),
                            _ => {}
                        }
                    }
                }
                if let (Some(pv), Some(nv)) = (pref, ns) {
                    seen.insert((pv, nv));
                }
            }
        }
    }

    for triple in graph.triples_for_predicate(DECLARE) {
        let s = triple.subject;
        if s == root {
            continue;
        }
        let o = triple.object;

        // remove the old triple <ontology, sh:declare, prefix>
        to_remove.push(triple.into());

        // Attempt to deduplicate using (prefix, namespace)
        if let Some(decl_node) = match o {
            TermRef::NamedNode(nn) => Some(NamedOrBlankNodeRef::NamedNode(nn)),
            TermRef::BlankNode(bn) => Some(NamedOrBlankNodeRef::BlankNode(bn)),
            _ => None,
        } {
            let mut pref: Option<String> = None;
            let mut ns: Option<String> = None;
            for t2 in graph.triples_for_subject(decl_node) {
                if t2.predicate == sh_prefix {
                    if let TermRef::Literal(l) = t2.object {
                        pref = Some(l.value().to_string());
                    }
                } else if t2.predicate == sh_namespace {
                    match t2.object {
                        TermRef::NamedNode(nn) => ns = Some(nn.as_str().to_string()),
                        TermRef::Literal(l) => ns = Some(l.value().to_string()),
                        _ => {}
                    }
                }
            }
            if let (Some(pv), Some(nv)) = (pref, ns) {
                if seen.insert((pv, nv)) {
                    // add a new triple <root, sh:declare, prefix>
                    let new_triple = TripleRef::new(root, DECLARE, o);
                    to_add.push(new_triple.into());
                }
                continue;
            }
        }

        // If we can't determine prefix/namespace, conservatively move it
        let new_triple = TripleRef::new(root, DECLARE, o);
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
pub fn remove_owl_imports_graph(graph: &mut Graph, ontologies_to_remove: Option<&[NamedNodeRef]>) {
    let to_remove: Vec<Triple> = graph
        .triples_for_predicate(IMPORTS)
        .filter_map(|triple| match triple.object {
            TermRef::NamedNode(obj) => {
                if ontologies_to_remove.is_none_or(|ontologies| ontologies.contains(&obj)) {
                    Some(triple.into())
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    // Remove the collected triples
    for triple in to_remove {
        graph.remove(triple.as_ref());
    }
}

/// Removes owl:Ontology declarations which are not the provided root
pub fn remove_ontology_declarations_graph(graph: &mut Graph, root: NamedOrBlankNodeRef) {
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

/// Rewrites all sh:prefixes in the graph to point to the provided root
pub fn rewrite_sh_prefixes(graph: &mut Dataset, root: NamedOrBlankNodeRef) {
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
    // move the sh:declare statements to the root ontology too, deduplicating by (sh:prefix, sh:namespace)
    let sh_prefix = NamedNodeRef::new_unchecked("http://www.w3.org/ns/shacl#prefix");
    let sh_namespace = NamedNodeRef::new_unchecked("http://www.w3.org/ns/shacl#namespace");
    let mut seen: HashSet<(String, String)> = HashSet::new();

    // Seed with any existing declarations on the root
    for q in graph.quads_for_predicate(DECLARE) {
        if q.subject == root {
            if let Some(decl_node) = match q.object {
                TermRef::NamedNode(nn) => Some(NamedOrBlankNodeRef::NamedNode(nn)),
                TermRef::BlankNode(bn) => Some(NamedOrBlankNodeRef::BlankNode(bn)),
                _ => None,
            } {
                let mut pref: Option<String> = None;
                let mut ns: Option<String> = None;
                for q2 in graph.quads_for_subject(decl_node) {
                    if q2.predicate == sh_prefix {
                        if let TermRef::Literal(l) = q2.object {
                            pref = Some(l.value().to_string());
                        }
                    } else if q2.predicate == sh_namespace {
                        match q2.object {
                            TermRef::NamedNode(nn) => ns = Some(nn.as_str().to_string()),
                            TermRef::Literal(l) => ns = Some(l.value().to_string()),
                            _ => {}
                        }
                    }
                }
                if let (Some(pv), Some(nv)) = (pref, ns) {
                    seen.insert((pv, nv));
                }
            }
        }
    }

    for quad in graph.quads_for_predicate(DECLARE) {
        let s = quad.subject;
        if s == root {
            continue;
        }
        let o = quad.object;
        let g = quad.graph_name;

        // remove the old quad <ontology, sh:declare, prefix>
        to_remove.push(quad.into());

        // Attempt to deduplicate using (prefix, namespace)
        if let Some(decl_node) = match o {
            TermRef::NamedNode(nn) => Some(NamedOrBlankNodeRef::NamedNode(nn)),
            TermRef::BlankNode(bn) => Some(NamedOrBlankNodeRef::BlankNode(bn)),
            _ => None,
        } {
            let mut pref: Option<String> = None;
            let mut ns: Option<String> = None;
            for q2 in graph.quads_for_subject(decl_node) {
                if q2.predicate == sh_prefix {
                    if let TermRef::Literal(l) = q2.object {
                        pref = Some(l.value().to_string());
                    }
                } else if q2.predicate == sh_namespace {
                    match q2.object {
                        TermRef::NamedNode(nn) => ns = Some(nn.as_str().to_string()),
                        TermRef::Literal(l) => ns = Some(l.value().to_string()),
                        _ => {}
                    }
                }
            }
            if let (Some(pv), Some(nv)) = (pref, ns) {
                if seen.insert((pv, nv)) {
                    let new_quad = QuadRef::new(root, DECLARE, o, g);
                    to_add.push(new_quad.into());
                }
                continue;
            }
        }

        // If we can't determine prefix/namespace, conservatively move it
        let new_quad = QuadRef::new(root, DECLARE, o, g);
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

/// Remove owl:imports statements from a graph. Can be helpful to do after computing the union of
/// all imports so that downstream tools do not attempt to fetch these graph dependencies
/// themselves. If ontologies_to_remove is provided, only remove owl:imports to those ontologies
pub fn remove_owl_imports(graph: &mut Dataset, ontologies_to_remove: Option<&[NamedNodeRef]>) {
    let to_remove: Vec<Quad> = graph
        .quads_for_predicate(IMPORTS)
        .filter_map(|quad| match quad.object {
            TermRef::NamedNode(obj) => {
                if ontologies_to_remove.is_none_or(|ontologies| ontologies.contains(&obj)) {
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

/// Removes owl:Ontology declarations which are not the provided root
pub fn remove_ontology_declarations(graph: &mut Dataset, root: NamedOrBlankNodeRef) {
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
