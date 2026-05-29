use std::borrow::Cow;
use std::collections::HashSet;
use std::iter::{empty, once};

use oxrdf::{BlankNode, Literal, NamedNode, Term};
use spareval::{
    InternalQuad, QueryEvaluationError, QueryEvaluator, QueryResults, QueryableDataset,
};
use spargebra::Query;

use crate::reader::{DecodedTerm, GraphRef, R5Error, R5tuFile};

/// A read-only SPARQL dataset view over an [`R5tuFile`].
///
/// Implements `spareval::QueryableDataset` directly against the on-disk
/// `.r5tu` representation. The internal term representation is the on-disk
/// **term id** (`u64`): `internal_quads_for_pattern` streams `(s, p, o)` ids
/// straight out of the triple blocks with no string decode, joins/`DISTINCT`/
/// `GROUP BY` hash `u64`s, and decoding to RDF terms (`externalize_term`)
/// happens only at final projection. Terms absent from the on-disk dictionary
/// (query constants, graph names, computed values) are interned into an
/// in-memory overflow table by [`R5tuFile::intern_decoded`].
///
/// Each physical graph (gid) is exposed as a distinct named graph using its
/// `graphname`; if two physical graphs share a name the SPARQL view will
/// surface their triples twice. Callers that want by-name (logical-graph)
/// deduplication should layer their own view on top.
///
/// A view can be scoped to an explicit subset of graph ids via
/// [`SparqlDatasetView::with_gids`].
#[derive(Clone, Copy, Debug)]
pub struct SparqlDatasetView<'a> {
    file: &'a R5tuFile,
    /// When `Some`, only these physical graph ids are visible to SPARQL.
    gids: Option<&'a [u64]>,
}

impl<'a> SparqlDatasetView<'a> {
    /// Construct a SPARQL view over an opened `.r5tu` file. The view borrows
    /// the file and is therefore as cheap to create as an immutable reference.
    pub fn new(file: &'a R5tuFile) -> Self {
        Self { file, gids: None }
    }

    /// Construct a view scoped to an explicit set of physical graph ids
    /// (gids). Only triples in those graph groups are visible to SPARQL —
    /// including `GRAPH ?g {}` enumeration and the default-graph union.
    ///
    /// Callers derive the gid set however they like, e.g.
    /// [`R5tuFile::enumerate_by_id`] to scope by source/dataset `id` or
    /// [`R5tuFile::enumerate_by_graphname`] to scope by graph name. The slice
    /// is borrowed for the life of the view.
    pub fn with_gids(file: &'a R5tuFile, gids: &'a [u64]) -> Self {
        Self {
            file,
            gids: Some(gids),
        }
    }

    /// The graphs visible to this view, optionally restricted to a single
    /// graph name.
    fn selected_graphs(&self, graphname: Option<&str>) -> Result<Vec<GraphRef>, R5Error> {
        match self.gids {
            Some(gids) => {
                let mut out = Vec::with_capacity(gids.len());
                for &gid in gids {
                    let graph = self.file.graphref_for_gid(gid)?;
                    if graphname.map(|name| graph.graphname == name).unwrap_or(true) {
                        out.push(graph);
                    }
                }
                Ok(out)
            }
            None => match graphname {
                Some(name) => self.file.enumerate_by_graphname(name),
                None => self.file.enumerate_all(),
            },
        }
    }

    fn quads_for_graph(
        file: &'a R5tuFile,
        graph: GraphRef,
        subject: Option<u64>,
        predicate: Option<u64>,
        object: Option<u64>,
    ) -> Box<dyn Iterator<Item = Result<InternalQuad<u64>, R5Error>> + 'a> {
        let graph_id = file.intern_decoded(&DecodedTerm::Iri(Cow::Borrowed(&graph.graphname)));
        let triples = match file.triples_ids(graph.gid) {
            Ok(triples) => triples,
            Err(error) => return Box::new(once(Err(error))),
        };
        Box::new(triples.filter_map(move |(s_id, p_id, o_id)| {
            if subject.is_some_and(|id| id != s_id)
                || predicate.is_some_and(|id| id != p_id)
                || object.is_some_and(|id| id != o_id)
            {
                return None;
            }
            Some(Ok(InternalQuad {
                subject: s_id,
                predicate: p_id,
                object: o_id,
                graph_name: Some(graph_id),
            }))
        }))
    }
}

impl<'a> QueryableDataset<'a> for SparqlDatasetView<'a> {
    type InternalTerm = u64;
    type Error = R5Error;

    #[allow(refining_impl_trait)]
    fn internal_quads_for_pattern(
        &self,
        subject: Option<&Self::InternalTerm>,
        predicate: Option<&Self::InternalTerm>,
        object: Option<&Self::InternalTerm>,
        graph_name: Option<Option<&Self::InternalTerm>>,
    ) -> Box<dyn Iterator<Item = Result<InternalQuad<Self::InternalTerm>, Self::Error>> + 'a> {
        let subject = subject.copied();
        let predicate = predicate.copied();
        let object = object.copied();

        // Resolve the optional graph-name filter to a concrete name. `None`
        // (any named graph) and `Some(None)` (default graph) both expand to the
        // union of all visible graphs — this dataset has no separate default
        // graph, so an unqualified pattern matches the union of named graphs.
        let graphname: Option<String> = match graph_name {
            None | Some(None) => None,
            Some(Some(&id)) => match self.file.externalize_id(id) {
                Ok(DecodedTerm::Iri(name)) => Some(name.into_owned()),
                // A non-IRI graph name (or an overflow id that isn't a stored
                // graph) matches nothing.
                Ok(_) => return Box::new(empty()),
                Err(error) => return Box::new(once(Err(error))),
            },
        };

        let graphs = match self.selected_graphs(graphname.as_deref()) {
            Ok(graphs) => graphs,
            Err(error) => return Box::new(once(Err(error))),
        };
        let file = self.file;
        Box::new(
            graphs
                .into_iter()
                .flat_map(move |graph| Self::quads_for_graph(file, graph, subject, predicate, object)),
        )
    }

    #[allow(refining_impl_trait)]
    fn internal_named_graphs(
        &self,
    ) -> Box<dyn Iterator<Item = Result<Self::InternalTerm, Self::Error>> + 'a> {
        let graphs = match self.selected_graphs(None) {
            Ok(graphs) => graphs,
            Err(error) => return Box::new(once(Err(error))),
        };
        let file = self.file;
        let mut seen = HashSet::new();
        let names = graphs
            .into_iter()
            .filter_map(|graph| {
                let id = file.intern_decoded(&DecodedTerm::Iri(Cow::Borrowed(&graph.graphname)));
                if seen.insert(id) {
                    Some(Ok(id))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        Box::new(names.into_iter())
    }

    fn contains_internal_graph_name(
        &self,
        graph_name: &Self::InternalTerm,
    ) -> Result<bool, Self::Error> {
        let name = match self.file.externalize_id(*graph_name)? {
            DecodedTerm::Iri(name) => name.into_owned(),
            _ => return Ok(false),
        };
        Ok(!self.selected_graphs(Some(&name))?.is_empty())
    }

    fn internalize_term(&self, term: Term) -> Result<Self::InternalTerm, Self::Error> {
        Ok(self.file.intern_decoded(&term_to_decoded(&term)))
    }

    fn externalize_term(&self, term: Self::InternalTerm) -> Result<Term, Self::Error> {
        decoded_to_term(self.file.externalize_id(term)?)
    }
}

/// Convert an oxrdf [`Term`] into a [`DecodedTerm`], normalizing `xsd:string`
/// typed literals to the plain (no-datatype) form so they match the on-disk
/// dictionary encoding.
pub(crate) fn term_to_decoded(term: &Term) -> DecodedTerm<'static> {
    match term {
        Term::NamedNode(node) => DecodedTerm::Iri(Cow::Owned(node.as_str().to_string())),
        Term::BlankNode(node) => DecodedTerm::BNode(Cow::Owned(node.as_str().to_string())),
        Term::Literal(literal) => {
            if let Some(language) = literal.language() {
                DecodedTerm::Literal {
                    lex: Cow::Owned(literal.value().to_string()),
                    dt: None,
                    lang: Some(Cow::Owned(language.to_string())),
                }
            } else {
                let datatype = literal.datatype();
                let datatype = if datatype.as_str() == "http://www.w3.org/2001/XMLSchema#string" {
                    None
                } else {
                    Some(Cow::Owned(datatype.as_str().to_string()))
                };
                DecodedTerm::Literal {
                    lex: Cow::Owned(literal.value().to_string()),
                    dt: datatype,
                    lang: None,
                }
            }
        }
    }
}

/// Convert a [`DecodedTerm`] back into an oxrdf [`Term`].
pub(crate) fn decoded_to_term(term: DecodedTerm<'_>) -> Result<Term, R5Error> {
    Ok(match term {
        DecodedTerm::Iri(value) => NamedNode::new(value.into_owned())
            .map_err(|_| R5Error::Invalid("invalid IRI term"))?
            .into(),
        DecodedTerm::BNode(value) => {
            let label = value.strip_prefix("_:").unwrap_or(value.as_ref());
            BlankNode::new(label.to_string())
                .map_err(|_| R5Error::Invalid("invalid blank node"))?
                .into()
        }
        DecodedTerm::Literal { lex, dt, lang } => {
            if let Some(dt) = dt {
                Literal::new_typed_literal(
                    lex.into_owned(),
                    NamedNode::new(dt.into_owned())
                        .map_err(|_| R5Error::Invalid("invalid datatype IRI"))?,
                )
                .into()
            } else if let Some(lang) = lang {
                Literal::new_language_tagged_literal(lex.into_owned(), lang.into_owned())
                    .map_err(|_| R5Error::Invalid("invalid language tag"))?
                    .into()
            } else {
                Literal::new_simple_literal(lex.into_owned()).into()
            }
        }
    })
}

impl R5tuFile {
    /// Returns a read-only SPARQL dataset view over this file.
    pub fn sparql_view(&self) -> SparqlDatasetView<'_> {
        SparqlDatasetView::new(self)
    }

    /// Returns a SPARQL dataset view scoped to an explicit subset of graph ids.
    ///
    /// See [`SparqlDatasetView::with_gids`].
    pub fn sparql_view_for_gids<'a>(&'a self, gids: &'a [u64]) -> SparqlDatasetView<'a> {
        SparqlDatasetView::with_gids(self, gids)
    }

    /// Evaluates a parsed SPARQL query directly against the rdf5d file.
    pub fn query<'a>(&'a self, query: &'a Query) -> Result<QueryResults<'a>, QueryEvaluationError> {
        QueryEvaluator::new()
            .prepare(query)
            .execute(self.sparql_view())
    }
}
