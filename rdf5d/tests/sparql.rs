#![cfg(feature = "sparql")]

use std::path::Path;

use rdf5d::{Quint, R5tuFile, Term, write_file};
use spareval::{QueryEvaluationError, QueryEvaluator, QueryResults};
use spargebra::SparqlParser;
use tempfile::tempdir;

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

#[test]
fn select_over_shared_graph_unions_ids() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("fixture.r5tu");
    build_fixture(&path);
    let file = R5tuFile::open(&path).expect("open fixture");
    let query = SparqlParser::new()
        .parse_query(
            "SELECT ?s WHERE {
               GRAPH <http://example.org/graph/shared> {
                 ?s <http://example.org/name> ?o
               }
             }",
        )
        .expect("query parses");

    let results = file.query(&query)?;
    let QueryResults::Solutions(solutions) = results else {
        panic!("expected solution results");
    };
    let mut subjects = solutions
        .map(|row| row.map(|solution| solution["s"].to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    subjects.sort();

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
    let file = R5tuFile::open(&path).expect("open fixture");

    let query = SparqlParser::new()
        .parse_query("SELECT ?s WHERE { ?s <http://example.org/name> ?o }")
        .expect("query parses");
    let results = file.query(&query)?;
    let QueryResults::Solutions(solutions) = results else {
        panic!("expected solution results");
    };
    let mut subjects = solutions
        .map(|row| row.map(|solution| solution["s"].to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    subjects.sort();

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
fn scoped_view_restricts_to_subset_of_gids() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("fixture.r5tu");
    build_fixture(&path);
    let file = R5tuFile::open(&path).expect("open fixture");

    // Scope to the "dataset:1" source: alice@shared and carol@other, but not
    // bob (which lives under dataset:2). The gid set is derived from the
    // id (source) index.
    let gids: Vec<u64> = file
        .enumerate_by_id("dataset:1")
        .expect("enumerate by id")
        .into_iter()
        .map(|graph| graph.gid)
        .collect();
    assert_eq!(gids.len(), 2, "dataset:1 spans two graph groups");

    let query = SparqlParser::new()
        .parse_query("SELECT ?s WHERE { ?s <http://example.org/name> ?o }")
        .expect("query parses");

    let results = QueryEvaluator::new()
        .prepare(&query)
        .execute(file.sparql_view_for_gids(&gids))?;
    let QueryResults::Solutions(solutions) = results else {
        panic!("expected solution results");
    };
    let mut subjects = solutions
        .map(|row| row.map(|solution| solution["s"].to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    subjects.sort();

    assert_eq!(
        subjects,
        vec!["<http://example.org/alice>", "<http://example.org/carol>"],
        "scoped view excludes triples from other sources"
    );
    Ok(())
}

#[test]
fn graph_variable_filter_uses_graph_name_terms() -> Result<(), QueryEvaluationError> {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("fixture.r5tu");
    build_fixture(&path);
    let file = R5tuFile::open(&path).expect("open fixture");

    let query = SparqlParser::new()
        .parse_query(
            "SELECT DISTINCT ?g WHERE {
               GRAPH ?g { ?s <http://example.org/name> ?o }
               FILTER(?g = <http://example.org/graph/shared>)
             }",
        )
        .expect("query parses");
    let results = file.query(&query)?;
    let QueryResults::Solutions(mut solutions) = results else {
        panic!("expected solution results");
    };
    let first = solutions.next().expect("one row")?;
    assert_eq!(first["g"].to_string(), "<http://example.org/graph/shared>");
    assert!(solutions.next().is_none());
    Ok(())
}
