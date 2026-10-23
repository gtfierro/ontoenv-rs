//! Shared graph contents for an uncommitted update. Parsed sources need set
//! semantics, but not the six query indexes of an OxRDF graph.

use oxigraph::model::{graph, Graph, Triple, TripleRef};
use std::collections::HashSet;
use std::slice;

pub(crate) enum StagedGraph {
    Triples(Vec<Triple>),
    Indexed(Box<Graph>),
}

impl StagedGraph {
    pub(crate) fn from_triples(triples: Vec<Triple>) -> Self {
        // Preserve input order using a temporary set of borrowed triples. The
        // serializer accepts duplicates, so staging must enforce set semantics.
        // Drop the set before compacting the owned vector; no terms are cloned.
        let keep: Vec<bool> = {
            let mut seen = HashSet::with_capacity(triples.len());
            triples.iter().map(|triple| seen.insert(triple)).collect()
        };
        let mut unique: Vec<Triple> = triples
            .into_iter()
            .zip(keep)
            .filter_map(|(triple, keep)| keep.then_some(triple))
            .collect();
        unique.shrink_to_fit();
        Self::Triples(unique)
    }

    pub(crate) fn to_graph(&self) -> Graph {
        match self {
            Self::Indexed(graph) => (**graph).clone(),
            Self::Triples(triples) => {
                let mut graph = Graph::new();
                for triple in triples {
                    graph.insert(triple.as_ref());
                }
                graph
            }
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = TripleRef<'_>> {
        match self {
            Self::Triples(triples) => StagedTriples::Triples(triples.iter()),
            Self::Indexed(graph) => StagedTriples::Indexed(graph.iter()),
        }
    }
}

enum StagedTriples<'a> {
    Triples(slice::Iter<'a, Triple>),
    Indexed(graph::Iter<'a>),
}

impl<'a> Iterator for StagedTriples<'a> {
    type Item = TripleRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Triples(triples) => triples.next().map(Triple::as_ref),
            Self::Indexed(triples) => triples.next(),
        }
    }
}
