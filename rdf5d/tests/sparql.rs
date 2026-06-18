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
