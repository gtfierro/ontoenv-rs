#![cfg(feature = "sparql")]

use std::path::Path;

use rdf5d::{Quint, Snapshot, Term, write_file};
use spareval::{QueryEvaluationError, QueryResults};
use spargebra::SparqlParser;
use tempfile::tempdir;

/// Two datasets feed the graph name `shared` (alice via dataset:1, bob via
/// dataset:2), so that name spans two physical graph groups. `other` holds
/// carol via dataset:1 only.
fn build_fixture(path: &Path) {
    let quints = vec![
        Quint {
            id: "dataset:1".into(),
            s: Term::Iri("http://example.org/alice".into()),
            p: Term::Iri("http://example.org/name".into()),
            o: Term::Literal {
                lex: "Alice".into(),
                dt: None,
                lang: None,
            },
            gname: "http://example.org/graph/shared".into(),
        },
        Quint {
            id: "dataset:2".into(),
            s: Term::Iri("http://example.org/bob".into()),
            p: Term::Iri("http://example.org/name".into()),
            o: Term::Literal {
                lex: "Bob".into(),
                dt: None,
                lang: None,
            },
            gname: "http://example.org/graph/shared".into(),
        },
        Quint {
            id: "dataset:1".into(),
            s: Term::Iri("http://example.org/carol".into()),
            p: Term::Iri("http://example.org/name".into()),
            o: Term::Literal {
                lex: "Carol".into(),
                dt: None,
                lang: None,
            },
            gname: "http://example.org/graph/other".into(),
        },
    ];
    write_file(path, &quints).expect("fixture written");
}

fn solution_subjects(results: QueryResults<'_>) -> Result<Vec<String>, QueryEvaluationError> {
    let QueryResults::Solutions(solutions) = results else {
        panic!("expected solution results");
    };
    let mut subjects = solutions
        .map(|row| row.map(|solution| solution["s"].to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    subjects.sort();
    Ok(subjects)
}

#[test]
fn graph_clause_unions_gids_sharing_a_name() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("fixture.r5tu");
    build_fixture(&path);
    let snap = Snapshot::open(&path).expect("open fixture");

    let mut query = SparqlParser::new()
        .parse_query(
            "SELECT ?s WHERE {
               GRAPH <http://example.org/graph/shared> {
                 ?s <http://example.org/name> ?o
               }
             }",
        )
        .expect("query parses");

    // The logical graph `shared` unions the two physical gids behind the name.
    let subjects = solution_subjects(snap.query(&mut query)?)?;
    assert_eq!(
        subjects,
        vec!["<http://example.org/alice>", "<http://example.org/bob>"]
    );
    Ok(())
}

#[test]
fn default_graph_is_union_of_named_graphs() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("fixture.r5tu");
    build_fixture(&path);
    let snap = Snapshot::open(&path).expect("open fixture");

    let mut query = SparqlParser::new()
        .parse_query("SELECT ?s WHERE { ?s <http://example.org/name> ?o }")
        .expect("query parses");
    let subjects = solution_subjects(snap.query(&mut query)?)?;
    assert_eq!(
        subjects,
        vec![
            "<http://example.org/alice>",
            "<http://example.org/bob>",
            "<http://example.org/carol>",
        ]
    );
    Ok(())
}

#[test]
fn logical_view_dedups_triples_across_shared_name() -> Result<(), QueryEvaluationError> {
    // The same triple is written under two datasets that share one graph name,
    // so it lives in two physical gids. The logical view must surface it once.
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("dup.r5tu");
    let triple = |id: &str| Quint {
        id: id.into(),
        s: Term::Iri("http://example.org/alice".into()),
        p: Term::Iri("http://example.org/name".into()),
        o: Term::Literal {
            lex: "Alice".into(),
            dt: None,
            lang: None,
        },
        gname: "http://example.org/graph/shared".into(),
    };
    write_file(&path, &[triple("dataset:1"), triple("dataset:2")]).expect("write");
    let snap = Snapshot::open(&path).expect("open");

    // No DISTINCT: a non-deduped (physical) view would bind ?s twice.
    let mut query = SparqlParser::new()
        .parse_query("SELECT ?s WHERE { ?s <http://example.org/name> ?o }")
        .expect("query parses");
    let subjects = solution_subjects(snap.query(&mut query)?)?;
    assert_eq!(subjects, vec!["<http://example.org/alice>"]);
    Ok(())
}

#[test]
fn graph_variable_filter_uses_graph_name_terms() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("fixture.r5tu");
    build_fixture(&path);
    let snap = Snapshot::open(&path).expect("open fixture");

    let mut query = SparqlParser::new()
        .parse_query(
            "SELECT DISTINCT ?g WHERE {
               GRAPH ?g { ?s <http://example.org/name> ?o }
               FILTER(?g = <http://example.org/graph/shared>)
             }",
        )
        .expect("query parses");
    let QueryResults::Solutions(mut solutions) = snap.query(&mut query)? else {
        panic!("expected solution results");
    };
    let first = solutions.next().expect("one row")?;
    assert_eq!(first["g"].to_string(), "<http://example.org/graph/shared>");
    assert!(solutions.next().is_none());
    Ok(())
}

// ── Property-path closure rewriting ──────────────────────────────────────────

/// Builds a fixture with a small class hierarchy:
///
///   A
///    └─ B
///        ├─ C
///        └─ D
///
/// A single unrelated class E (no subClassOf edges) is also present to
/// confirm the closure evaluator correctly skips it.
fn build_closure_fixture(path: &Path) {
    let subclass = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
    let class = |iri: &str| Quint {
        id: "data".into(),
        s: Term::Iri(iri.into()),
        p: Term::Iri("http://www.w3.org/1999/02/22-rdf-syntax-ns#type".into()),
        o: Term::Iri("http://www.w3.org/2002/07/owl#Class".into()),
        gname: "http://example.org/graph/onto".into(),
    };
    let edge = |s: &str, o: &str| Quint {
        id: "data".into(),
        s: Term::Iri(s.into()),
        p: Term::Iri(subclass.into()),
        o: Term::Iri(o.into()),
        gname: "http://example.org/graph/onto".into(),
    };
    let quints = vec![
        class("http://example.org/A"),
        class("http://example.org/B"),
        class("http://example.org/C"),
        class("http://example.org/D"),
        class("http://example.org/E"),
        edge("http://example.org/B", "http://example.org/A"),
        edge("http://example.org/C", "http://example.org/B"),
        edge("http://example.org/D", "http://example.org/B"),
    ];
    write_file(path, &quints).expect("closure fixture written");
}

fn solution_values(results: QueryResults<'_>, var: &str) -> Vec<String> {
    let QueryResults::Solutions(solutions) = results else {
        panic!("expected solution results");
    };
    let mut values: Vec<String> = solutions
        .map(|row| row.map(|solution| solution[var].to_string()))
        .collect::<Result<Vec<_>, _>>()
        .expect("collect solutions");
    values.sort();
    values.dedup();
    values
}

/// Both variables unbound: ?x subClassOf+ ?y. The rewriter bails out because
/// neither endpoint is bound, falls through to spareval. The result should be
/// the transitive closure minus the reflexive entries (C, D → B, C → A, D → A,
/// B → A).
#[test]
fn closure_both_variables_bails_out_to_spareval() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("closure.r5tu");
    build_closure_fixture(&path);
    let snap = Snapshot::open(&path).expect("open fixture");

    let mut query = SparqlParser::new()
        .parse_query(
            "SELECT ?s ?o WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#subClassOf>+ ?o }",
        )
        .expect("query parses");
    let subjects = solution_values(snap.query(&mut query)?, "s");
    let objects = solution_values(snap.query(&mut query)?, "o");

    // Forward: B→A, C→B, D→B  (direct edges from the fixture).
    // The P+ closure also yields C→A and D→A (transitive).
    // spareval may also produce A → nothing since it's a leaf.
    assert!(
        subjects.contains(&"<http://example.org/C>".to_string()),
        "C should be a subject in closure"
    );
    assert!(
        objects.contains(&"<http://example.org/A>".to_string()),
        "A should be an object in closure"
    );
    // E has no subClassOf edges and should not appear.
    assert!(
        !subjects.contains(&"<http://example.org/E>".to_string()),
        "E should not appear as subject"
    );
    Ok(())
}

/// Subject bound: ex:C subClassOf+ ?o  →  forward closure of C: [B, A]
#[test]
fn closure_subject_bound_forward() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("closure.r5tu");
    build_closure_fixture(&path);
    let snap = Snapshot::open(&path).expect("open fixture");

    let mut query = SparqlParser::new()
        .parse_query(
            "SELECT ?o WHERE { <http://example.org/C> <http://www.w3.org/2000/01/rdf-schema#subClassOf>+ ?o }",
        )
        .expect("query parses");
    let objects = solution_values(snap.query(&mut query)?, "o");
    assert_eq!(objects, vec!["<http://example.org/A>", "<http://example.org/B>"]);
    Ok(())
}

/// Subject bound with P* (reflexive): ex:C subClassOf* ?o → [C, B, A]
#[test]
fn closure_subject_bound_star_includes_self() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("closure.r5tu");
    build_closure_fixture(&path);
    let snap = Snapshot::open(&path).expect("open fixture");

    let mut query = SparqlParser::new()
        .parse_query(
            "SELECT ?o WHERE { <http://example.org/C> <http://www.w3.org/2000/01/rdf-schema#subClassOf>* ?o }",
        )
        .expect("query parses");
    let objects = solution_values(snap.query(&mut query)?, "o");
    assert_eq!(
        objects,
        vec![
            "<http://example.org/A>",
            "<http://example.org/B>",
            "<http://example.org/C>",
        ]
    );
    Ok(())
}

/// Object bound: ?s subClassOf+ ex:A  →  reverse closure of A: [B, C, D]
#[test]
fn closure_object_bound_reverse() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("closure.r5tu");
    build_closure_fixture(&path);
    let snap = Snapshot::open(&path).expect("open fixture");

    let mut query = SparqlParser::new()
        .parse_query(
            "SELECT ?s WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#subClassOf>+ <http://example.org/A> }",
        )
        .expect("query parses");
    let subjects = solution_values(snap.query(&mut query)?, "s");
    assert_eq!(
        subjects,
        vec![
            "<http://example.org/B>",
            "<http://example.org/C>",
            "<http://example.org/D>",
        ]
    );
    Ok(())
}

/// Both const, reachable: ex:C subClassOf+ ex:A  →  true (empty BGP)
#[test]
fn closure_both_const_reachable_returns_empty_bgp() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("closure.r5tu");
    build_closure_fixture(&path);
    let snap = Snapshot::open(&path).expect("open fixture");

    // ASK query: true if C is a subclass of A
    let mut query = SparqlParser::new()
        .parse_query(
            "ASK { <http://example.org/C> <http://www.w3.org/2000/01/rdf-schema#subClassOf>+ <http://example.org/A> }",
        )
        .expect("query parses");
    let QueryResults::Boolean(ask_result) = snap.query(&mut query)? else {
        panic!("expected boolean result");
    };
    assert!(ask_result, "C should reach A via subClassOf+");
    Ok(())
}

/// Both const, unreachable: ex:C subClassOf+ ex:E  →  false (empty Values)
#[test]
fn closure_both_const_unreachable_returns_false() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("closure.r5tu");
    build_closure_fixture(&path);
    let snap = Snapshot::open(&path).expect("open fixture");

    let mut query = SparqlParser::new()
        .parse_query(
            "ASK { <http://example.org/C> <http://www.w3.org/2000/01/rdf-schema#subClassOf>+ <http://example.org/E> }",
        )
        .expect("query parses");
    let QueryResults::Boolean(ask_result) = snap.query(&mut query)? else {
        panic!("expected boolean result");
    };
    assert!(!ask_result, "C should NOT reach E via subClassOf+");
    Ok(())
}

/// Both const unreachable with P*: also false (self-ref only, but E is
/// unrelated — no subClassOf self-loop in the data).
#[test]
fn closure_star_both_const_unreachable() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("closure.r5tu");
    build_closure_fixture(&path);
    let snap = Snapshot::open(&path).expect("open fixture");

    let mut query = SparqlParser::new()
        .parse_query(
            "ASK { <http://example.org/E> <http://www.w3.org/2000/01/rdf-schema#subClassOf>* <http://example.org/A> }",
        )
        .expect("query parses");
    let QueryResults::Boolean(ask_result) = snap.query(&mut query)? else {
        panic!("expected boolean result");
    };
    assert!(!ask_result, "E should NOT reach A via subClassOf*");
    Ok(())
}

/// Reversed path: ?s ^subClassOf+ ex:C  →  forward closure of C: [B, A]
#[test]
fn closure_reversed_path() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("closure.r5tu");
    build_closure_fixture(&path);
    let snap = Snapshot::open(&path).expect("open fixture");

    let mut query = SparqlParser::new()
        .parse_query(
            "SELECT ?s WHERE { ?s ^<http://www.w3.org/2000/01/rdf-schema#subClassOf>+ <http://example.org/C> }",
        )
        .expect("query parses");
    let subjects = solution_values(snap.query(&mut query)?, "s");
    // ?s ^subClassOf+ C  ≡  C subClassOf+ ?s  →  C's ancestors: B, A
    assert_eq!(subjects, vec!["<http://example.org/A>", "<http://example.org/B>"]);
    Ok(())
}

/// Predicate NOT in the closure index (a random property) — must fall back
/// to spareval evaluation without crashing, returning correct results for the
/// direct edges (P+ on a non-closure predicate can still be evaluated by
/// spareval against the underlying store).
#[test]
fn closure_predicate_not_in_index_falls_back() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("closure.r5tu");
    // Add a chain with a custom predicate
    let custom_pred = "http://example.org/customPred";
    let quints = vec![
        Quint {
            id: "data".into(),
            s: Term::Iri("http://example.org/X".into()),
            p: Term::Iri(custom_pred.into()),
            o: Term::Iri("http://example.org/Y".into()),
            gname: "http://example.org/graph/onto".into(),
        },
        Quint {
            id: "data".into(),
            s: Term::Iri("http://example.org/Y".into()),
            p: Term::Iri(custom_pred.into()),
            o: Term::Iri("http://example.org/Z".into()),
            gname: "http://example.org/graph/onto".into(),
        },
    ];
    write_file(&path, &quints).expect("custom predicate fixture");
    let snap = Snapshot::open(&path).expect("open fixture");

    let mut query = SparqlParser::new()
        .parse_query(
            &format!("SELECT ?o WHERE {{ <http://example.org/X> <{custom_pred}>+ ?o }}"),
        )
        .expect("query parses");
    let objects = solution_values(snap.query(&mut query)?, "o");
    assert_eq!(
        objects,
        vec!["<http://example.org/Y>", "<http://example.org/Z>"]
    );
    Ok(())
}
