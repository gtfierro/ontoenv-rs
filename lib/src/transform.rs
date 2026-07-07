//! Provides functions for transforming RDF graphs and datasets within the OntoEnv context.
//! This includes rewriting SHACL prefixes and removing OWL imports or ontology declarations.

use anyhow::{anyhow, Result};

use crate::consts::{
    DECLARE, IMPORTS, ONTOLOGY, PREFIXES, SH_NAMESPACE, SH_PREFIX, TYPE, VERSION_IRI,
};
use oxigraph::model::{
    Dataset, Graph, NamedNode, NamedNodeRef, NamedOrBlankNode, NamedOrBlankNodeRef, Quad, QuadRef,
    Term, TermRef, Triple, TripleRef,
};
use std::collections::HashMap;

/// Extract the `(sh:prefix, sh:namespace)` pair from a declaration node in a `Graph`.
fn extract_decl_prefix_ns_graph(
    graph: &Graph,
    decl_node: NamedOrBlankNodeRef,
) -> (Option<String>, Option<String>) {
    let mut pref: Option<String> = None;
    let mut ns: Option<String> = None;
    for t in graph.triples_for_subject(decl_node) {
        if t.predicate == SH_PREFIX {
            if let TermRef::Literal(l) = t.object {
                pref = Some(l.value().to_string());
            }
        } else if t.predicate == SH_NAMESPACE {
            match t.object {
                TermRef::NamedNode(nn) => ns = Some(nn.as_str().to_string()),
                TermRef::Literal(l) => ns = Some(l.value().to_string()),
                _ => {}
            }
        }
    }
    (pref, ns)
}

const CONFLICT_HINT: &str = "Fix the conflict or disable sh:prefixes rewriting \
    (CLI: `--no-rewrite-sh-prefixes`, Python: `rewrite_sh_prefixes=False`, \
    Rust: `rewrite_sh_prefixes=Some(false)`).";

fn check_and_insert(seen: &mut HashMap<String, String>, pv: String, nv: String) -> Result<bool> {
    if let Some(existing_ns) = seen.get(&pv) {
        if *existing_ns != nv {
            return Err(anyhow!(
                "Conflicting sh:prefix \"{pv}\": namespace \"{existing_ns}\" vs \"{nv}\". {CONFLICT_HINT}"
            ));
        }
        // Same prefix, same namespace — deduplicate
        return Ok(false);
    }
    seen.insert(pv, nv);
    Ok(true)
}

/// Rewrites all `sh:prefixes` links in a graph so they point at `root`, moving each `sh:declare`
/// block onto `root` and deduplicating declarations by `(sh:prefix, sh:namespace)`.
pub fn rewrite_sh_prefixes_graph(graph: &mut Graph, root: NamedOrBlankNodeRef) -> Result<()> {
    let mut to_remove: Vec<Triple> = vec![];
    let mut to_add: Vec<Triple> = vec![];

    for triple in graph.triples_for_predicate(PREFIXES) {
        let s = triple.subject;
        to_remove.push(triple.into());
        to_add.push(TripleRef::new(s, PREFIXES, root).into());
    }

    let mut seen: HashMap<String, String> = HashMap::new();

    // Seed with any existing declarations on the root
    for t in graph.triples_for_predicate(DECLARE) {
        if t.subject == root {
            if let Some(decl_node) = match t.object {
                TermRef::NamedNode(nn) => Some(NamedOrBlankNodeRef::NamedNode(nn)),
                TermRef::BlankNode(bn) => Some(NamedOrBlankNodeRef::BlankNode(bn)),
                _ => None,
            } {
                let (pref, ns) = extract_decl_prefix_ns_graph(graph, decl_node);
                if let (Some(pv), Some(nv)) = (pref, ns) {
                    check_and_insert(&mut seen, pv, nv)?;
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
        to_remove.push(triple.into());

        if let Some(decl_node) = match o {
            TermRef::NamedNode(nn) => Some(NamedOrBlankNodeRef::NamedNode(nn)),
            TermRef::BlankNode(bn) => Some(NamedOrBlankNodeRef::BlankNode(bn)),
            _ => None,
        } {
            let (pref, ns) = extract_decl_prefix_ns_graph(graph, decl_node);
            if let (Some(pv), Some(nv)) = (pref, ns) {
                if check_and_insert(&mut seen, pv, nv)? {
                    to_add.push(TripleRef::new(root, DECLARE, o).into());
                }
                continue;
            }
        }

        // If we can't determine prefix/namespace, conservatively move it
        to_add.push(TripleRef::new(root, DECLARE, o).into());
    }

    for triple in to_remove {
        graph.remove(triple.as_ref());
    }
    for triple in to_add {
        graph.insert(triple.as_ref());
    }
    Ok(())
}

/// Remove owl:imports statements from a graph. Can be helpful to do after computing the union of
/// all imports so that downstream tools do not attempt to fetch these graph dependencies
/// themselves. If ontologies_to_remove is provided, only remove owl:imports to those ontologies
pub fn remove_owl_imports_graph(graph: &mut Graph, ontologies_to_remove: Option<&[NamedNodeRef]>) {
    // Strip owl:imports so consumers do not refetch already-resolved dependencies.
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

/// Rewrites every occurrence of `old_iri` in `graph` to `new_iri`, covering:
/// - triples where `old_iri` is the **subject** (owl:Ontology declaration, metadata, sh:declare links)
/// - triples where `old_iri` is the **object** (sh:prefixes targets, self-referential owl:imports, etc.)
///
/// Predicate rewriting is intentionally omitted because ontology IRIs never appear as predicates.
pub fn rename_ontology_iri_graph(graph: &mut Graph, old_iri: NamedNodeRef, new_iri: NamedNodeRef) {
    let new_iri_owned: NamedNode = new_iri.into();
    let new_subj: NamedOrBlankNode = new_iri_owned.clone().into();
    let new_term: Term = new_iri_owned.clone().into();
    let old_subject = NamedOrBlankNodeRef::NamedNode(old_iri);
    let old_term = TermRef::NamedNode(old_iri);
    let old_term_owned: Term = NamedNode::from(old_iri).into();

    let mut to_remove: Vec<Triple> = Vec::new();
    let mut to_add: Vec<Triple> = Vec::new();

    // Triples where old_iri is the subject.
    for triple_ref in graph.triples_for_subject(old_subject) {
        let owned: Triple = triple_ref.into();
        // owl:versionIRI values are stable identifiers; leave the object as-is.
        let new_obj = if owned.object == old_term_owned && owned.predicate != VERSION_IRI {
            new_term.clone()
        } else {
            owned.object.clone()
        };
        to_add.push(Triple::new(
            new_subj.clone(),
            owned.predicate.clone(),
            new_obj,
        ));
        to_remove.push(owned);
    }

    // Triples where old_iri is the object (skip those already handled as subject,
    // and skip owl:versionIRI — version identifiers must not be silently changed).
    for triple_ref in graph.triples_for_object(old_term) {
        if triple_ref.subject == old_subject {
            continue; // already rewritten above
        }
        if triple_ref.predicate == VERSION_IRI {
            continue; // version IRI is a stable identifier; do not rewrite
        }
        let owned: Triple = triple_ref.into();
        to_add.push(Triple::new(
            owned.subject.clone(),
            owned.predicate.clone(),
            new_term.clone(),
        ));
        to_remove.push(owned);
    }

    for triple in &to_remove {
        graph.remove(triple.as_ref());
    }
    for triple in &to_add {
        graph.insert(triple.as_ref());
    }
}

/// Removes owl:Ontology declarations which are not the provided root
pub fn remove_ontology_declarations_graph(graph: &mut Graph, root: NamedOrBlankNodeRef) {
    // Keep only the root ontology declaration to avoid multiple "main" declarations.
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

fn extract_decl_prefix_ns_dataset(
    ds: &Dataset,
    decl_node: NamedOrBlankNodeRef,
) -> (Option<String>, Option<String>) {
    let mut pref: Option<String> = None;
    let mut ns: Option<String> = None;
    for q in ds.quads_for_subject(decl_node) {
        if q.predicate == SH_PREFIX {
            if let TermRef::Literal(l) = q.object {
                pref = Some(l.value().to_string());
            }
        } else if q.predicate == SH_NAMESPACE {
            match q.object {
                TermRef::NamedNode(nn) => ns = Some(nn.as_str().to_string()),
                TermRef::Literal(l) => ns = Some(l.value().to_string()),
                _ => {}
            }
        }
    }
    (pref, ns)
}

/** Rewrites all `sh:prefixes` entries in the dataset to point at `root`, relocating `sh:declare`
blocks onto `root` and deduplicating declarations by `(sh:prefix, sh:namespace)`. */
pub fn rewrite_sh_prefixes_dataset(graph: &mut Dataset, root: NamedOrBlankNodeRef) -> Result<()> {
    let mut to_remove: Vec<Quad> = vec![];
    let mut to_add: Vec<Quad> = vec![];

    for quad in graph.quads_for_predicate(PREFIXES) {
        let s = quad.subject;
        let g = quad.graph_name;
        to_remove.push(quad.into());
        to_add.push(QuadRef::new(s, PREFIXES, root, g).into());
    }

    let mut seen: HashMap<String, String> = HashMap::new();

    // Seed with any existing declarations on the root
    for q in graph.quads_for_predicate(DECLARE) {
        if q.subject == root {
            if let Some(decl_node) = match q.object {
                TermRef::NamedNode(nn) => Some(NamedOrBlankNodeRef::NamedNode(nn)),
                TermRef::BlankNode(bn) => Some(NamedOrBlankNodeRef::BlankNode(bn)),
                _ => None,
            } {
                let (pref, ns) = extract_decl_prefix_ns_dataset(graph, decl_node);
                if let (Some(pv), Some(nv)) = (pref, ns) {
                    check_and_insert(&mut seen, pv, nv)?;
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
        to_remove.push(quad.into());

        if let Some(decl_node) = match o {
            TermRef::NamedNode(nn) => Some(NamedOrBlankNodeRef::NamedNode(nn)),
            TermRef::BlankNode(bn) => Some(NamedOrBlankNodeRef::BlankNode(bn)),
            _ => None,
        } {
            let (pref, ns) = extract_decl_prefix_ns_dataset(graph, decl_node);
            if let (Some(pv), Some(nv)) = (pref, ns) {
                if check_and_insert(&mut seen, pv, nv)? {
                    to_add.push(QuadRef::new(root, DECLARE, o, g).into());
                }
                continue;
            }
        }

        // If we can't determine prefix/namespace, conservatively move it
        to_add.push(QuadRef::new(root, DECLARE, o, g).into());
    }

    for quad in to_remove {
        graph.remove(quad.as_ref());
    }
    for quad in to_add {
        graph.insert(quad.as_ref());
    }
    Ok(())
}

/// Remove owl:imports statements from a dataset. Can be helpful to do after computing the union of
/// all imports so that downstream tools do not attempt to fetch these graph dependencies
/// themselves. If ontologies_to_remove is provided, only remove owl:imports to those ontologies
pub fn remove_owl_imports_dataset(
    graph: &mut Dataset,
    ontologies_to_remove: Option<&[NamedNodeRef]>,
) {
    // Dataset variant to prune owl:imports quads for resolved ontologies.
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
    // Forward to the newer name while keeping API compatibility.
    remove_ontology_declarations_dataset(graph, root)
}

/// Backwards-compat wrapper; prefer remove_owl_imports_dataset
pub fn remove_owl_imports(graph: &mut Dataset, ontologies_to_remove: Option<&[NamedNodeRef]>) {
    // Forward to the newer name while keeping API compatibility.
    remove_owl_imports_dataset(graph, ontologies_to_remove)
}

/// Removes owl:Ontology declarations in a dataset which are not the provided root
pub fn remove_ontology_declarations_dataset(graph: &mut Dataset, root: NamedOrBlankNodeRef) {
    // Dataset variant to collapse ontology declarations onto the root subject.
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
        rewrite_sh_prefixes_dataset(&mut ds, root).expect("rewrite should succeed");

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

    #[test]
    fn errors_on_conflicting_sh_prefix_across_graphs() {
        let mut ds = Dataset::new();

        let ont1 = NamedNode::new("http://example.com/ont1").unwrap();
        let ont2 = NamedNode::new("http://example.com/ont2").unwrap();
        let g1 = NamedNode::new("http://example.com/graph1").unwrap();
        let g2 = NamedNode::new("http://example.com/graph2").unwrap();

        // Same prefix "ex" but different namespaces
        add_decl(&mut ds, &ont1, &g1, "ex", "http://example.com/ns/one#");
        add_decl(&mut ds, &ont2, &g2, "ex", "http://example.com/ns/two#");

        let root = NamedOrBlankNodeRef::NamedNode(ont1.as_ref());
        let result = rewrite_sh_prefixes_dataset(&mut ds, root);
        assert!(result.is_err(), "Expected conflict error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Conflicting sh:prefix \"ex\""),
            "Error message should mention the conflicting prefix: {msg}"
        );
    }
}
