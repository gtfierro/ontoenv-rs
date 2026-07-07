//! SPARQL evaluation over a [`Snapshot`].
//!
//! [`SparqlView`] adapts a snapshot to [`spareval::QueryableDataset`] using
//! **logical** (by-name) graph semantics: the gids that share a graph name are
//! unioned and deduplicated, so SPARQL sees each name as a single named graph.
//! Evaluation runs entirely on on-disk term ids ([`Snapshot::scan`]); terms are
//! decoded to RDF only at final projection.
//!
//! [`Snapshot::query`] additionally rewrites `P+`/`P*` property paths whose
//! predicate is in the snapshot's precomputed closure index into materialized
//! `VALUES` blocks before handing the query to spareval.

use std::borrow::Cow;
use std::iter::{empty, once};

use oxrdf::{BlankNode, Literal, NamedNode, Term};
use spareval::{
    InternalQuad, QueryEvaluationError, QueryEvaluator, QueryResults, QueryableDataset,
};
use spargebra::Query;
use spargebra::algebra::{GraphPattern, PropertyPathExpression};
use spargebra::term::{GroundTerm, TermPattern, Variable};

use crate::reader::{DecodedTerm, R5Error};
use crate::snapshot::{Pattern, Scope, Snapshot};

/// A read-only SPARQL dataset view over a [`Snapshot`], using logical (by-name)
/// graph semantics. Cheap to create — it just borrows the snapshot.
#[derive(Clone, Copy, Debug)]
pub struct SparqlView<'a> {
    snapshot: &'a Snapshot,
}

impl<'a> SparqlView<'a> {
    pub fn new(snapshot: &'a Snapshot) -> Self {
        Self { snapshot }
    }

    fn graph_id(&self, name: &str) -> u64 {
        self.snapshot
            .file()
            .intern_decoded(&DecodedTerm::Iri(Cow::Borrowed(name)))
    }
}

impl<'a> QueryableDataset<'a> for SparqlView<'a> {
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
        let pat = Pattern {
            s: subject.copied(),
            p: predicate.copied(),
            o: object.copied(),
        };
        let snapshot = self.snapshot;

        // Resolve the optional graph-name filter to a concrete name. `None`
        // (any named graph) and `Some(None)` (default graph) both expand to the
        // union of all logical graphs — this dataset has no separate default
        // graph.
        let name: Option<String> = match graph_name {
            None | Some(None) => None,
            Some(Some(&id)) => match snapshot.file().externalize_id(id) {
                Ok(DecodedTerm::Iri(name)) => Some(name.into_owned()),
                // A non-IRI graph name (or an overflow id that isn't a stored
                // graph) matches nothing.
                Ok(_) => return Box::new(empty()),
                Err(error) => return Box::new(once(Err(error))),
            },
        };

        match name {
            Some(name) => {
                if !snapshot.has_graph(&name) {
                    return Box::new(empty());
                }
                let graph_id = self.graph_id(&name);
                // `scan` copies the gids and borrows only the snapshot, so the
                // returned iterator does not retain `name` — we can stream.
                Box::new(
                    snapshot
                        .scan(pat, Scope::ByName(name.as_str()))
                        .map(move |hit| {
                            hit.map(|m| InternalQuad {
                                subject: m.s,
                                predicate: m.p,
                                object: m.o,
                                graph_name: Some(graph_id),
                            })
                        }),
                )
            }
            None => Box::new(snapshot.graph_names().flat_map(move |name| {
                let graph_id = snapshot
                    .file()
                    .intern_decoded(&DecodedTerm::Iri(Cow::Borrowed(name)));
                snapshot.scan(pat, Scope::ByName(name)).map(move |hit| {
                    hit.map(|m| InternalQuad {
                        subject: m.s,
                        predicate: m.p,
                        object: m.o,
                        graph_name: Some(graph_id),
                    })
                })
            })),
        }
    }

    #[allow(refining_impl_trait)]
    fn internal_named_graphs(
        &self,
    ) -> Box<dyn Iterator<Item = Result<Self::InternalTerm, Self::Error>> + 'a> {
        let ids: Vec<_> = self
            .snapshot
            .graph_names()
            .map(|name| Ok(self.graph_id(name)))
            .collect();
        Box::new(ids.into_iter())
    }

    fn contains_internal_graph_name(
        &self,
        graph_name: &Self::InternalTerm,
    ) -> Result<bool, Self::Error> {
        let name = match self.snapshot.file().externalize_id(*graph_name)? {
            DecodedTerm::Iri(name) => name.into_owned(),
            _ => return Ok(false),
        };
        Ok(self.snapshot.has_graph(&name))
    }

    fn internalize_term(&self, term: Term) -> Result<Self::InternalTerm, Self::Error> {
        Ok(self.snapshot.file().intern_decoded(&term_to_decoded(&term)))
    }

    fn externalize_term(&self, term: Self::InternalTerm) -> Result<Term, Self::Error> {
        decoded_to_term(self.snapshot.file().externalize_id(term)?)
    }
}

impl Snapshot {
    /// A logical (by-name) SPARQL dataset view over this snapshot.
    pub fn sparql_view(&self) -> SparqlView<'_> {
        SparqlView::new(self)
    }

    /// Rewrite `P+`/`P*` property paths whose predicate is in the precomputed
    /// closure index into materialized `VALUES`/BGP blocks. A no-op when the
    /// snapshot has no closure data.
    pub fn rewrite_query(&self, query: &mut Query) {
        PClosRewriter::new(self).rewrite_query(query);
    }

    /// Rewrite closures, then evaluate the query against this snapshot.
    pub fn query<'a>(
        &'a self,
        query: &'a mut Query,
    ) -> Result<QueryResults<'a>, QueryEvaluationError> {
        self.rewrite_query(query);
        QueryEvaluator::new()
            .prepare(query)
            .execute(self.sparql_view())
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

/// Rewrites `?x P+ ?y` / `?x P* ?y` graph patterns whose predicate is in the
/// snapshot's precomputed closure index, substituting precomputed `VALUES`
/// blocks (or BGPs) for them.
///
/// Bail-out cases: predicate not in the closure index; both endpoints are
/// variables; path is anything other than a direct `ZeroOrMore`/`OneOrMore` of
/// a single `NamedNode` (optionally reversed). In all bail-outs the original
/// pattern is left intact and spareval evaluates the property path itself.
struct PClosRewriter<'a> {
    snapshot: &'a Snapshot,
}

impl<'a> PClosRewriter<'a> {
    fn new(snapshot: &'a Snapshot) -> Self {
        Self { snapshot }
    }

    fn enabled(&self) -> bool {
        self.snapshot.has_closures()
    }

    fn predicate_id(&self, iri: &str) -> Option<u64> {
        self.snapshot
            .file()
            .term_id(&DecodedTerm::Iri(Cow::Borrowed(iri)))
    }

    fn named_node(&self, term_id: u64) -> Option<spargebra::term::NamedNode> {
        match self.snapshot.file().decoded_term(term_id).ok()? {
            DecodedTerm::Iri(s) => spargebra::term::NamedNode::new(s.into_owned()).ok(),
            _ => None,
        }
    }

    fn rewrite_query(&self, query: &mut Query) {
        if !self.enabled() {
            return;
        }
        match query {
            Query::Select { pattern, .. }
            | Query::Construct { pattern, .. }
            | Query::Describe { pattern, .. }
            | Query::Ask { pattern, .. } => self.rewrite_pattern(pattern),
        }
    }

    fn rewrite_pattern(&self, pat: &mut GraphPattern) {
        // Recurse first so inner Path nodes are still reachable.
        match pat {
            GraphPattern::Path { .. } => {}
            GraphPattern::Join { left, right }
            | GraphPattern::Union { left, right }
            | GraphPattern::Minus { left, right } => {
                self.rewrite_pattern(left);
                self.rewrite_pattern(right);
            }
            GraphPattern::LeftJoin { left, right, .. } => {
                self.rewrite_pattern(left);
                self.rewrite_pattern(right);
            }
            GraphPattern::Filter { inner, .. }
            | GraphPattern::Graph { inner, .. }
            | GraphPattern::Extend { inner, .. }
            | GraphPattern::OrderBy { inner, .. }
            | GraphPattern::Project { inner, .. }
            | GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. }
            | GraphPattern::Group { inner, .. }
            | GraphPattern::Service { inner, .. } => self.rewrite_pattern(inner),
            GraphPattern::Bgp { .. } | GraphPattern::Values { .. } => {}
            // Catch-all for feature-gated variants (e.g. Lateral). Leave alone.
            #[allow(unreachable_patterns)]
            _ => {}
        }
        if let GraphPattern::Path {
            subject,
            path,
            object,
        } = pat
            && let Some(replacement) = self.rewrite_path(subject, path, object)
        {
            *pat = replacement;
        }
    }

    /// Try to compute a replacement `GraphPattern` for a path triple. Returns
    /// `None` to leave the pattern untouched.
    fn rewrite_path(
        &self,
        subject: &TermPattern,
        path: &PropertyPathExpression,
        object: &TermPattern,
    ) -> Option<GraphPattern> {
        // Extract `(NamedNode(p), include_reflexive_for_star, reversed)` for the
        // paths we can handle: ZeroOrMore(P), OneOrMore(P), and the reverse
        // variants ^P+ / ^P*.
        let (named, include_reflexive, reversed) = match path {
            PropertyPathExpression::OneOrMore(inner) => match inner.as_ref() {
                PropertyPathExpression::NamedNode(p) => (p, false, false),
                PropertyPathExpression::Reverse(inner2) => match inner2.as_ref() {
                    PropertyPathExpression::NamedNode(p) => (p, false, true),
                    _ => return None,
                },
                _ => return None,
            },
            PropertyPathExpression::ZeroOrMore(inner) => match inner.as_ref() {
                PropertyPathExpression::NamedNode(p) => (p, true, false),
                PropertyPathExpression::Reverse(inner2) => match inner2.as_ref() {
                    PropertyPathExpression::NamedNode(p) => (p, true, true),
                    _ => return None,
                },
                _ => return None,
            },
            _ => return None,
        };
        let p_id = self.predicate_id(named.as_str())?;

        // After reversal, the "forward" lookup direction corresponds to the
        // *object* side of the SPARQL path: `?s ^P+ ?o` ≡ `?o P+ ?s`.
        let (left, right) = if reversed {
            (object, subject)
        } else {
            (subject, object)
        };

        match (left, right) {
            // const P+ ?var -> forward closure of const, bind ?var
            (TermPattern::NamedNode(c), TermPattern::Variable(var)) => {
                let c_id = self.predicate_id(c.as_str())?;
                let mut answers = self
                    .snapshot
                    .closure_forward(p_id, c_id)
                    .unwrap_or_default();
                if include_reflexive {
                    answers.push(c_id);
                    answers.sort();
                    answers.dedup();
                }
                Some(self.values_for_var(var, &answers))
            }
            // ?var P+ const -> reverse closure of const, bind ?var
            (TermPattern::Variable(var), TermPattern::NamedNode(c)) => {
                let c_id = self.predicate_id(c.as_str())?;
                let mut answers = self
                    .snapshot
                    .closure_reverse(p_id, c_id)
                    .unwrap_or_default();
                if include_reflexive {
                    answers.push(c_id);
                    answers.sort();
                    answers.dedup();
                }
                Some(self.values_for_var(var, &answers))
            }
            // const1 P+ const2 -> membership check, replace with empty or true
            (TermPattern::NamedNode(s), TermPattern::NamedNode(o)) => {
                let s_id = self.predicate_id(s.as_str())?;
                let o_id = self.predicate_id(o.as_str())?;
                let answers = self
                    .snapshot
                    .closure_forward(p_id, s_id)
                    .unwrap_or_default();
                let reachable =
                    answers.binary_search(&o_id).is_ok() || (include_reflexive && s_id == o_id);
                if reachable {
                    // Always-true: an empty BGP matches a single empty row.
                    Some(GraphPattern::Bgp { patterns: vec![] })
                } else {
                    // Always-false: a Values with no rows.
                    Some(GraphPattern::Values {
                        variables: vec![],
                        bindings: vec![],
                    })
                }
            }
            // Both endpoints variable — leave the path to spareval. The full
            // closure table can be unbounded, so a materialized VALUES block is
            // rarely a win here.
            (TermPattern::Variable(_), TermPattern::Variable(_)) => None,
            // Anything else (Literal, BlankNode, Triple) — leave alone.
            _ => None,
        }
    }

    fn values_for_var(&self, var: &Variable, ids: &[u64]) -> GraphPattern {
        let mut bindings: Vec<Vec<Option<GroundTerm>>> = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(nn) = self.named_node(*id) {
                bindings.push(vec![Some(GroundTerm::NamedNode(nn))]);
            }
        }
        GraphPattern::Values {
            variables: vec![var.clone()],
            bindings,
        }
    }
}
