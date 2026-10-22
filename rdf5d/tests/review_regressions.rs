use rdf5d::{
    reader::{IntegrityMode, OpenOptions, R5Error, R5tuFile},
    writer::{Quint, Term, write_file},
};

fn fixture(path: &std::path::Path) {
    let iri = |value: &str| Term::Iri(format!("http://ex/{value}"));
    write_file(
        path,
        &[
            Quint {
                id: "source".into(),
                gname: "http://ex/g".into(),
                s: iri("s"),
                p: iri("p"),
                o: iri("o1"),
            },
            Quint {
                id: "source".into(),
                gname: "http://ex/g".into(),
                s: iri("s"),
                p: iri("p"),
                o: iri("o2"),
            },
        ],
    )
    .unwrap();
}

fn structural(path: &std::path::Path) -> R5tuFile {
    R5tuFile::open_with_options(
        path,
        OpenOptions {
            integrity: IntegrityMode::Structural,
            prefer_mmap: false,
        },
    )
    .unwrap()
}

#[test]
fn rejects_missing_footer_and_unknown_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.r5tu");
    fixture(&path);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        R5tuFile::open(&path),
        Err(R5Error::Invalid("unsupported format version"))
    ));
    bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
    bytes.truncate(bytes.len() - 16);
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(R5tuFile::open(&path), Err(R5Error::Corrupt(_))));
}

#[test]
fn corrupt_dictionary_and_term_offsets_return_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.r5tu");
    fixture(&path);
    let original = std::fs::read(&path).unwrap();
    let file = structural(&path);

    let mut bytes = original.clone();
    let dict = file.section(rdf5d::header::SectionKind::IdDict).unwrap();
    let base = dict.off as usize;
    let offsets = u64::from_le_bytes(bytes[base + 20..base + 28].try_into().unwrap()) as usize;
    bytes[offsets..offsets + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let damaged = structural(&path);
    assert!(matches!(damaged.enumerate_all(), Err(R5Error::Corrupt(_))));

    let mut bytes = original;
    let term = file.section(rdf5d::header::SectionKind::TermDict).unwrap();
    let base = term.off as usize;
    let offsets = u64::from_le_bytes(bytes[base + 25..base + 33].try_into().unwrap()) as usize;
    bytes[offsets..offsets + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let damaged = structural(&path);
    assert!(matches!(damaged.decoded_term(0), Err(R5Error::Corrupt(_))));
}

#[test]
fn corrupt_posting_and_pair_offsets_return_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.r5tu");
    fixture(&path);
    let original = std::fs::read(&path).unwrap();
    let file = structural(&path);

    let mut bytes = original.clone();
    let section = file.section(rdf5d::header::SectionKind::IdxId2Gid).unwrap();
    let base = section.off as usize;
    bytes[base..base + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        structural(&path).enumerate_by_id("source"),
        Err(R5Error::Corrupt(_))
    ));

    let mut bytes = original;
    let section = file
        .section(rdf5d::header::SectionKind::IdxPair2Gid)
        .unwrap();
    let base = section.off as usize;
    bytes[base + 8..base + 16].copy_from_slice(&(section.off + section.len + 1).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        structural(&path).resolve_gid("source", "http://ex/g"),
        Err(R5Error::Corrupt(_))
    ));
}

#[test]
fn lazy_triples_report_later_corruption_at_its_position() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.r5tu");
    fixture(&path);
    let mut bytes = std::fs::read(&path).unwrap();
    let file = structural(&path);
    let gdir = file.section(rdf5d::header::SectionKind::GDir).unwrap();
    let row = gdir.off as usize + 16;
    let block = u32::from_le_bytes(bytes[row + 8..row + 12].try_into().unwrap()) as usize;
    let raw_len = u32::from_le_bytes(bytes[block + 1..block + 5].try_into().unwrap()) as usize;
    bytes[block + 5 + raw_len - 1] = 0x80;
    std::fs::write(&path, &bytes).unwrap();
    let damaged = structural(&path);
    assert!(matches!(damaged.triples_ids(0), Err(R5Error::Corrupt(_))));
    let mut lazy = damaged.triples_ids_lazy(0).unwrap();
    assert!(lazy.next().unwrap().is_ok());
    assert!(matches!(lazy.next(), Some(Err(R5Error::Corrupt(_)))));
    assert!(lazy.next().is_none());
}

#[cfg(feature = "idx")]
#[test]
fn view_contains_propagates_invalid_gid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.r5tu");
    fixture(&path);
    let snapshot = rdf5d::Snapshot::open(&path).unwrap();
    let view = rdf5d::View::from_gids(&snapshot, vec![u64::MAX]);
    assert!(matches!(
        view.contains(None, None, None),
        Err(R5Error::Invalid(_))
    ));
}

#[cfg(feature = "sparql")]
#[test]
fn view_query_scopes_named_graphs_and_deduplicates_shared_name() {
    use rdf5d::{Snapshot, View};
    use spareval::QueryResults;
    use spargebra::SparqlParser;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.r5tu");
    let iri = |value: &str| Term::Iri(format!("http://ex/{value}"));
    let triple = |id: &str, graph: &str| Quint {
        id: id.into(),
        gname: graph.into(),
        s: iri("s"),
        p: iri("p"),
        o: iri("o"),
    };
    write_file(
        &path,
        &[
            triple("source1", "http://ex/g1"),
            triple("source2", "http://ex/g1"),
            triple("source3", "http://ex/g2"),
        ],
    )
    .unwrap();
    let snapshot = Snapshot::open(&path).unwrap();
    let view = View::from_names(&snapshot, &["http://ex/g1"]);
    let mut query = SparqlParser::new()
        .parse_query("SELECT ?g ?s WHERE { GRAPH ?g { ?s <http://ex/p> <http://ex/o> } }")
        .unwrap();
    let QueryResults::Solutions(rows) = view.query(&mut query).unwrap() else {
        panic!("expected solutions")
    };
    let rows = rows.collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["g"].to_string(), "<http://ex/g1>");
}

#[cfg(feature = "sparql")]
#[test]
fn view_property_path_stays_within_scoped_graphs() {
    use rdf5d::{Snapshot, View};
    use spareval::QueryResults;
    use spargebra::SparqlParser;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paths.r5tu");
    let edge = |id: &str, graph: &str, subject: &str, object: &str| Quint {
        id: id.into(),
        gname: graph.into(),
        s: Term::Iri(subject.into()),
        p: Term::Iri("http://www.w3.org/2000/01/rdf-schema#subClassOf".into()),
        o: Term::Iri(object.into()),
    };
    write_file(
        &path,
        &[
            edge("source1", "http://ex/g1", "http://ex/a", "http://ex/b"),
            edge("source2", "http://ex/g2", "http://ex/b", "http://ex/c"),
        ],
    )
    .unwrap();
    let snapshot = Snapshot::open(&path).unwrap();
    let view = View::from_names(&snapshot, &["http://ex/g1"]);
    let mut query = SparqlParser::new().parse_query(
        "SELECT ?o WHERE { <http://ex/a> <http://www.w3.org/2000/01/rdf-schema#subClassOf>+ ?o }"
    ).unwrap();
    let QueryResults::Solutions(rows) = view.query(&mut query).unwrap() else {
        panic!("expected solutions")
    };
    let rows = rows.collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["o"].to_string(), "<http://ex/b>");
}

#[cfg(feature = "sparql")]
#[test]
fn graph_clause_property_path_does_not_cross_view_graphs() {
    use rdf5d::{Snapshot, View};
    use spareval::QueryResults;
    use spargebra::SparqlParser;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("graph_paths.r5tu");
    let edge = |id: &str, graph: &str, subject: &str, object: &str| Quint {
        id: id.into(),
        gname: graph.into(),
        s: Term::Iri(subject.into()),
        p: Term::Iri("http://www.w3.org/2000/01/rdf-schema#subClassOf".into()),
        o: Term::Iri(object.into()),
    };
    write_file(
        &path,
        &[
            edge("source1", "http://ex/g1", "http://ex/a", "http://ex/b"),
            edge("source2", "http://ex/g2", "http://ex/b", "http://ex/c"),
        ],
    )
    .unwrap();
    let snapshot = Snapshot::open(&path).unwrap();
    let view = View::from_names(&snapshot, &["http://ex/g1", "http://ex/g2"]);
    let path_pattern = "<http://ex/a> <http://www.w3.org/2000/01/rdf-schema#subClassOf>+ ?o";

    let mut graph_query = SparqlParser::new()
        .parse_query(&format!(
            "SELECT ?o WHERE {{ GRAPH <http://ex/g1> {{ {path_pattern} }} }}"
        ))
        .unwrap();
    let QueryResults::Solutions(rows) = view.query(&mut graph_query).unwrap() else {
        panic!("expected solutions")
    };
    let objects: Vec<_> = rows.map(|row| row.unwrap()["o"].to_string()).collect();
    assert_eq!(objects, vec!["<http://ex/b>"]);

    let mut snapshot_graph_query = SparqlParser::new()
        .parse_query(&format!(
            "SELECT ?o WHERE {{ GRAPH <http://ex/g1> {{ {path_pattern} }} }}"
        ))
        .unwrap();
    let QueryResults::Solutions(rows) = snapshot.query(&mut snapshot_graph_query).unwrap() else {
        panic!("expected solutions")
    };
    let objects: Vec<_> = rows.map(|row| row.unwrap()["o"].to_string()).collect();
    assert_eq!(objects, vec!["<http://ex/b>"]);

    let mut variable_graph_query = SparqlParser::new()
        .parse_query(&format!(
            "SELECT ?g ?o WHERE {{ GRAPH ?g {{ {path_pattern} }} }}"
        ))
        .unwrap();
    let QueryResults::Solutions(rows) = view.query(&mut variable_graph_query).unwrap() else {
        panic!("expected solutions")
    };
    let pairs: Vec<_> = rows
        .map(|row| {
            let row = row.unwrap();
            (row["g"].to_string(), row["o"].to_string())
        })
        .collect();
    assert_eq!(
        pairs,
        vec![("<http://ex/g1>".into(), "<http://ex/b>".into())]
    );

    let mut union_query = SparqlParser::new()
        .parse_query(&format!("SELECT ?o WHERE {{ {path_pattern} }}"))
        .unwrap();
    let QueryResults::Solutions(rows) = view.query(&mut union_query).unwrap() else {
        panic!("expected solutions")
    };
    let mut objects: Vec<_> = rows.map(|row| row.unwrap()["o"].to_string()).collect();
    objects.sort();
    assert_eq!(objects, vec!["<http://ex/b>", "<http://ex/c>"]);
}

#[cfg(feature = "idx")]
#[test]
fn grouped_scan_reports_contexts_once_and_honors_scope() {
    use rdf5d::{Pattern, Scope, Snapshot, View};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("grouped.r5tu");
    let triple = |id: &str, graph: &str, object: &str| Quint {
        id: id.into(),
        gname: graph.into(),
        s: Term::Iri("http://ex/s".into()),
        p: Term::Iri("http://ex/p".into()),
        o: Term::Iri(object.into()),
    };
    write_file(
        &path,
        &[
            triple("source1", "http://ex/g1", "http://ex/o"),
            triple("source2", "http://ex/g1", "http://ex/o"),
            triple("source3", "http://ex/g2", "http://ex/o"),
            triple("source3", "http://ex/g2", "http://ex/other"),
        ],
    )
    .unwrap();
    let snapshot = Arc::new(Snapshot::open(&path).unwrap());
    let all: Vec<_> = snapshot
        .grouped_scan(Pattern::ANY, Scope::All)
        .unwrap()
        .collect();
    assert_eq!(all.len(), 2);
    let shared = all.iter().find(|row| row.gids.len() == 3).unwrap();
    assert_eq!(shared.gids.len(), 3);

    let view = View::from_names(&snapshot, &["http://ex/g1"]);
    for pattern in [
        Pattern {
            s: Some(shared.s),
            ..Pattern::ANY
        },
        Pattern {
            p: Some(shared.p),
            ..Pattern::ANY
        },
        Pattern {
            o: Some(shared.o),
            ..Pattern::ANY
        },
        Pattern {
            p: Some(shared.p),
            o: Some(shared.o),
            ..Pattern::ANY
        },
    ] {
        let matches: Vec<_> = view.scan(pattern).collect::<rdf5d::Result<_>>().unwrap();
        assert_eq!(matches.len(), 1);
    }
    let scoped: Vec<_> = snapshot
        .grouped_scan(Pattern::ANY, Scope::Gids(view.gids()))
        .unwrap()
        .collect();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].gids.len(), 2);
}

#[test]
fn legacy_explicit_xsd_string_can_be_found_as_plain_literal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy-literal.r5tu");
    write_file(
        &path,
        &[Quint {
            id: "source".into(),
            gname: "http://ex/g".into(),
            s: Term::Iri("http://ex/s".into()),
            p: Term::Iri("http://ex/p".into()),
            o: Term::Literal {
                lex: "hello".into(),
                dt: Some("http://www.w3.org/2001/XMLSchema#string".into()),
                lang: None,
            },
        }],
    )
    .unwrap();
    let file = R5tuFile::open(&path).unwrap();
    let plain = rdf5d::DecodedTerm::Literal {
        lex: "hello".into(),
        dt: None,
        lang: None,
    };
    assert!(file.term_id(&plain).is_some());
}
