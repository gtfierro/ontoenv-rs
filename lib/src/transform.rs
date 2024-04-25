use crate::consts::{DECLARE, IMPORTS, ONTOLOGY, PREFIXES, TYPE};
use oxigraph::model::{
    Dataset, Graph, GraphName, NamedNode, NamedNodeRef, NamedOrBlankNode, Quad, QuadRef, SubjectRef,
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

/// Remove owl:imports statements from a graph. Can be helpful to do after computing the union of
/// all imports so that downstream tools do not attempt to fetch these graph dependencies
/// themselves
pub fn remove_owl_imports(graph: &mut Dataset) {
    // remove owl:imports
    let mut to_remove: Vec<Quad> = vec![];
    for quad in graph.quads_for_predicate(IMPORTS) {
        to_remove.push(quad.into());
    }
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
