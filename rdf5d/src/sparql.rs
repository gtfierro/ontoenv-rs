use std::borrow::Cow;
use std::collections::HashSet;
use std::iter::{empty, once};

use oxrdf::{BlankNode, Literal, NamedNode, Term};
use spareval::{
    InternalQuad, QueryEvaluationError, QueryEvaluator, QueryResults, QueryableDataset,
};
use spargebra::Query;

use crate::reader::{DecodedTerm, R5Error, R5tuFile};

/// A read-only SPARQL dataset view over an [`R5tuFile`].
#[derive(Clone, Copy, Debug)]
pub struct SparqlDatasetView<'a> {
    file: &'a R5tuFile,
}

impl<'a> SparqlDatasetView<'a> {
    pub fn new(file: &'a R5tuFile) -> Self {
        Self { file }
    }

    fn graph_term(name: String) -> DecodedTerm<'a> {
        DecodedTerm::Iri(Cow::Owned(name))
    }

    fn quads_for_gid(
        file: &'a R5tuFile,
        gid: u64,
        graph_term: DecodedTerm<'a>,
        subject: Option<DecodedTerm<'a>>,
        predicate: Option<DecodedTerm<'a>>,
        object: Option<DecodedTerm<'a>>,
    ) -> Box<dyn Iterator<Item = Result<InternalQuad<DecodedTerm<'a>>, R5Error>> + 'a> {
        let triples = match file.triples_ids(gid) {
            Ok(triples) => triples,
            Err(error) => return Box::new(once(Err(error))),
        };
        Box::new(triples.filter_map(move |(s_id, p_id, o_id)| {
            let subject_term = match file.decoded_term(s_id) {
                Ok(term) => term,
                Err(error) => return Some(Err(error)),
            };
            if subject
                .as_ref()
                .is_some_and(|expected| expected != &subject_term)
            {
                return None;
            }

            let predicate_term = match file.decoded_term(p_id) {
                Ok(term) => term,
                Err(error) => return Some(Err(error)),
            };
            if predicate
                .as_ref()
                .is_some_and(|expected| expected != &predicate_term)
            {
                return None;
            }

            let object_term = match file.decoded_term(o_id) {
                Ok(term) => term,
                Err(error) => return Some(Err(error)),
            };
            if object
                .as_ref()
                .is_some_and(|expected| expected != &object_term)
            {
                return None;
            }

            Some(Ok(InternalQuad {
                subject: subject_term,
                predicate: predicate_term,
                object: object_term,
                graph_name: Some(graph_term.clone()),
            }))
        }))
    }
}

impl<'a> QueryableDataset<'a> for SparqlDatasetView<'a> {
    type InternalTerm = DecodedTerm<'a>;
    type Error = R5Error;

    #[allow(refining_impl_trait)]
    fn internal_quads_for_pattern(
        &self,
        subject: Option<&Self::InternalTerm>,
        predicate: Option<&Self::InternalTerm>,
        object: Option<&Self::InternalTerm>,
        graph_name: Option<Option<&Self::InternalTerm>>,
    ) -> Box<dyn Iterator<Item = Result<InternalQuad<Self::InternalTerm>, Self::Error>> + 'a> {
        let Some(graph_filter) = graph_name else {
            let graphs = match self.file.enumerate_all() {
                Ok(graphs) => graphs,
                Err(error) => return Box::new(once(Err(error))),
            };
            let file = self.file;
            let subject = subject.cloned();
            let predicate = predicate.cloned();
            let object = object.cloned();
            return Box::new(graphs.into_iter().flat_map(move |graph| {
                Self::quads_for_gid(
                    file,
                    graph.gid,
                    Self::graph_term(graph.graphname),
                    subject.clone(),
                    predicate.clone(),
                    object.clone(),
                )
            }));
        };

        let Some(graph_term) = graph_filter else {
            let graphs = match self.file.enumerate_all() {
                Ok(graphs) => graphs,
                Err(error) => return Box::new(once(Err(error))),
            };
            let file = self.file;
            let subject = subject.cloned();
            let predicate = predicate.cloned();
            let object = object.cloned();
            return Box::new(graphs.into_iter().flat_map(move |graph| {
                Self::quads_for_gid(
                    file,
                    graph.gid,
                    Self::graph_term(graph.graphname),
                    subject.clone(),
                    predicate.clone(),
                    object.clone(),
                )
            }));
        };
        let DecodedTerm::Iri(graph_name) = graph_term else {
            return Box::new(empty());
        };
        let graphs = match self.file.enumerate_by_graphname(graph_name.as_ref()) {
            Ok(graphs) => graphs,
            Err(error) => return Box::new(once(Err(error))),
        };
        let file = self.file;
        let subject = subject.cloned();
        let predicate = predicate.cloned();
        let object = object.cloned();
        Box::new(graphs.into_iter().flat_map(move |graph| {
            Self::quads_for_gid(
                file,
                graph.gid,
                Self::graph_term(graph.graphname),
                subject.clone(),
                predicate.clone(),
                object.clone(),
            )
        }))
    }

    #[allow(refining_impl_trait)]
    fn internal_named_graphs(
        &self,
    ) -> Box<dyn Iterator<Item = Result<Self::InternalTerm, Self::Error>> + 'a> {
        let graphs = match self.file.enumerate_all() {
            Ok(graphs) => graphs,
            Err(error) => return Box::new(once(Err(error))),
        };
        let mut seen = HashSet::new();
        let names = graphs
            .into_iter()
            .filter_map(|graph| {
                if seen.insert(graph.graphname.clone()) {
                    Some(Ok(Self::graph_term(graph.graphname)))
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
        let DecodedTerm::Iri(graph_name) = graph_name else {
            return Ok(false);
        };
        Ok(!self
            .file
            .enumerate_by_graphname(graph_name.as_ref())?
            .is_empty())
    }

    fn internalize_term(&self, term: Term) -> Result<Self::InternalTerm, Self::Error> {
        Ok(match term {
            Term::NamedNode(node) => DecodedTerm::Iri(Cow::Owned(node.into_string())),
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
                    let datatype = if datatype.as_str() == "http://www.w3.org/2001/XMLSchema#string"
                    {
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
        })
    }

    fn externalize_term(&self, term: Self::InternalTerm) -> Result<Term, Self::Error> {
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
}

impl R5tuFile {
    /// Returns a read-only SPARQL dataset view over this file.
    pub fn sparql_view(&self) -> SparqlDatasetView<'_> {
        SparqlDatasetView::new(self)
    }

    /// Evaluates a parsed SPARQL query directly against the rdf5d file.
    pub fn query<'a>(&'a self, query: &'a Query) -> Result<QueryResults<'a>, QueryEvaluationError> {
        QueryEvaluator::new()
            .prepare(query)
            .execute(self.sparql_view())
    }
}
