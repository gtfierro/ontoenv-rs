//! Provides functions for transforming RDF graphs and datasets within the OntoEnv context.
//! This includes rewriting SHACL prefixes and removing OWL imports or ontology declarations.

use crate::consts::{DECLARE, IMPORTS, ONTOLOGY, PREFIXES, TYPE};
use oxigraph::model::{
    Dataset, Graph, NamedNodeRef, NamedOrBlankNodeRef, Quad, QuadRef, TermRef, Triple, TripleRef,
};
use std::collections::HashSet;

/// Rewrites all `sh:prefixes` links in a graph so they point at `root`, moving each `sh:declare`
/// block onto `root` and deduplicating declarations by `(sh:prefix, sh:namespace)`.
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

/** Rewrites all `sh:prefixes` entries in the dataset to point at `root`, relocating `sh:declare`
blocks onto `root` and deduplicating declarations by `(sh:prefix, sh:namespace)`. */
pub fn rewrite_sh_prefixes_dataset(graph: &mut Dataset, root: NamedOrBlankNodeRef) {
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

/// Remove owl:imports statements from a dataset. Can be helpful to do after computing the union of
/// all imports so that downstream tools do not attempt to fetch these graph dependencies
/// themselves. If ontologies_to_remove is provided, only remove owl:imports to those ontologies
pub fn remove_owl_imports_dataset(
    graph: &mut Dataset,
    ontologies_to_remove: Option<&[NamedNodeRef]>,
) {
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

/// Backwards-compat wrapper; prefer remove_ontology_declarations_dataset
pub fn remove_ontology_declarations(graph: &mut Dataset, root: NamedOrBlankNodeRef) {
    remove_ontology_declarations_dataset(graph, root)
}

/// Backwards-compat wrapper; prefer remove_owl_imports_dataset
pub fn remove_owl_imports(graph: &mut Dataset, ontologies_to_remove: Option<&[NamedNodeRef]>) {
    remove_owl_imports_dataset(graph, ontologies_to_remove)
}

/// Removes owl:Ontology declarations in a dataset which are not the provided root
pub fn remove_ontology_declarations_dataset(graph: &mut Dataset, root: NamedOrBlankNodeRef) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::model::{
        BlankNode, GraphName, Literal, NamedNode, NamedNodeRef, NamedOrBlankNode, Term,
    };
    use std::collections::HashSet;

    fn add_decl(
        ds: &mut Dataset,
        subject: &NamedNode,
        graph_name: &NamedNode,
        prefix: &str,
        namespace: &str,
    ) {
        let decl_bnode = BlankNode::default();
        // subject sh:declare _:decl
        ds.insert(&Quad::new(
            NamedOrBlankNode::from(subject.clone()),
            DECLARE.into_owned(),
            Term::from(decl_bnode.clone()),
            GraphName::NamedNode(graph_name.clone()),
        ));

        // _:decl sh:prefix "prefix"
        let sh_prefix = NamedNode::new("http://www.w3.org/ns/shacl#prefix").unwrap();
        ds.insert(&Quad::new(
            NamedOrBlankNode::from(decl_bnode.clone()),
            sh_prefix,
            Term::from(Literal::new_simple_literal(prefix)),
            GraphName::NamedNode(graph_name.clone()),
        ));

        // _:decl sh:namespace <namespace>
        let sh_namespace = NamedNode::new("http://www.w3.org/ns/shacl#namespace").unwrap();
        let ns_node = NamedNode::new(namespace).unwrap();
        ds.insert(&Quad::new(
            NamedOrBlankNode::from(decl_bnode),
            sh_namespace,
            Term::from(ns_node),
            GraphName::NamedNode(graph_name.clone()),
        ));
    }

    #[test]
    fn deduplicates_sh_declare_by_prefix_and_namespace_across_graphs() {
        // Two graphs, one imports the other. Each has 3 declarations:
        // - one identical pair across both graphs (same prefix+namespace)
        // - one pair with same namespace but different prefixes
        // - one fully different
        let mut ds = Dataset::new();

        let ont1 = NamedNode::new("http://example.com/ont1").unwrap();
        let ont2 = NamedNode::new("http://example.com/ont2").unwrap();
        let g1 = NamedNode::new("http://example.com/graph1").unwrap();
        let g2 = NamedNode::new("http://example.com/graph2").unwrap();

        // ont1 imports ont2 (for scenario realism)
        let owl_imports = NamedNode::new("http://www.w3.org/2002/07/owl#imports").unwrap();
        ds.insert(&Quad::new(
            NamedOrBlankNode::from(ont1.clone()),
            owl_imports,
            Term::from(ont2.clone()),
            GraphName::NamedNode(g1.clone()),
        ));

        // Graph 1 declarations
        add_decl(
            &mut ds,
            &ont1,
            &g1,
            "cmn",
            "http://example.com/ns/identical#",
        ); // identical across graphs
        add_decl(&mut ds, &ont1, &g1, "ex", "http://example.com/ns/same#"); // same namespace, different prefixes
        add_decl(&mut ds, &ont1, &g1, "only1", "http://example.com/ns/only1#"); // unique to graph1

        // Graph 2 declarations
        add_decl(
            &mut ds,
            &ont2,
            &g2,
            "cmn",
            "http://example.com/ns/identical#",
        ); // identical across graphs
        add_decl(&mut ds, &ont2, &g2, "ex2", "http://example.com/ns/same#"); // same namespace, different prefixes
        add_decl(&mut ds, &ont2, &g2, "only2", "http://example.com/ns/only2#"); // unique to graph2

        // Rewrite to root (ont1), deduplicating by (prefix, namespace)
        let root = NamedOrBlankNodeRef::NamedNode(ont1.as_ref());
        rewrite_sh_prefixes_dataset(&mut ds, root);

        // Count root declarations and ensure there are none left on non-root subjects
        let declare_ref = NamedNodeRef::new_unchecked("http://www.w3.org/ns/shacl#declare");

        let root_count = ds
            .quads_for_predicate(declare_ref)
            .filter(|q| q.subject == root)
            .count();
        let non_root_count = ds
            .quads_for_predicate(declare_ref)
            .filter(|q| q.subject != root)
            .count();

        assert_eq!(root_count, 5, "Expected 5 unique (prefix,namespace) pairs");
        assert_eq!(
            non_root_count, 0,
            "All sh:declare triples should be moved to the root"
        );

        // Verify the exact set of (prefix, namespace) pairs on the root
        let sh_prefix_ref = NamedNodeRef::new_unchecked("http://www.w3.org/ns/shacl#prefix");
        let sh_namespace_ref = NamedNodeRef::new_unchecked("http://www.w3.org/ns/shacl#namespace");

        let mut pairs: HashSet<(String, String)> = HashSet::new();
        for q in ds
            .quads_for_predicate(declare_ref)
            .filter(|q| q.subject == root)
        {
            // Follow the declaration node to collect prefix+namespace
            if let Some(decl_node) = match q.object {
                TermRef::NamedNode(nn) => Some(NamedOrBlankNodeRef::NamedNode(nn)),
                TermRef::BlankNode(bn) => Some(NamedOrBlankNodeRef::BlankNode(bn)),
                _ => None,
            } {
                let mut pref: Option<String> = None;
                let mut ns: Option<String> = None;
                for q2 in ds.quads_for_subject(decl_node) {
                    if q2.predicate == sh_prefix_ref {
                        if let TermRef::Literal(l) = q2.object {
                            pref = Some(l.value().to_string());
                        }
                    } else if q2.predicate == sh_namespace_ref {
                        match q2.object {
                            TermRef::NamedNode(nn) => ns = Some(nn.as_str().to_string()),
                            TermRef::Literal(l) => ns = Some(l.value().to_string()),
                            _ => {}
                        }
                    }
                }
                if let (Some(p), Some(n)) = (pref, ns) {
                    pairs.insert((p, n));
                } else {
                    panic!("Root declaration missing sh:prefix or sh:namespace");
                }
            } else {
                panic!("sh:declare object was not a named or blank node");
            }
        }

        let expected: HashSet<(String, String)> = [
            (
                "cmn".to_string(),
                "http://example.com/ns/identical#".to_string(),
            ),
            ("ex".to_string(), "http://example.com/ns/same#".to_string()),
            ("ex2".to_string(), "http://example.com/ns/same#".to_string()),
            (
                "only1".to_string(),
                "http://example.com/ns/only1#".to_string(),
            ),
            (
                "only2".to_string(),
                "http://example.com/ns/only2#".to_string(),
            ),
        ]
        .into_iter()
        .collect();

        assert_eq!(pairs, expected);
    }
}
