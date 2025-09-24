use rdf5d::{
    reader::R5tuFile,
    replace_graph,
    writer::{Quint, Term, write_file},
};

#[test]
fn replace_entire_graph_preserves_others() {
    // Initial dataset: two graphs under the same graphname
    let quints = vec![
        Quint {
            id: "src/A".into(),
            gname: "g".into(),
            s: Term::Iri("ex:s1".into()),
            p: Term::Iri("ex:p".into()),
            o: Term::Iri("ex:o1".into()),
        },
        Quint {
            id: "src/B".into(),
            gname: "g".into(),
            s: Term::Iri("ex:s2".into()),
            p: Term::Iri("ex:p".into()),
            o: Term::Iri("ex:o2".into()),
        },
    ];

    let mut in_path = std::env::temp_dir();
    in_path.push("update_in.r5tu");
    write_file(&in_path, &quints).expect("write input");

    // New content for graph (src/A, g)
    let new_triples = vec![
        (
            Term::Iri("ex:s1".into()),
            Term::Iri("ex:p2".into()),
            Term::Literal {
                lex: "v2".into(),
                dt: None,
                lang: Some("en".into()),
            },
        ),
        (
            Term::Iri("ex:s3".into()),
            Term::Iri("ex:p3".into()),
            Term::Iri("ex:o3".into()),
        ),
    ];

    let mut out_path = std::env::temp_dir();
    out_path.push("update_out.r5tu");
    replace_graph(&in_path, &out_path, "src/A", "g", &new_triples).expect("replace ok");

    // Validate output
    let f = R5tuFile::open(&out_path).expect("open out");
    // Graphs by graphname still two
    let gs = f.enumerate_by_graphname("g").expect("enum g");
    assert_eq!(gs.len(), 2);

    // src/A now has 2 triples with the new predicate/object
    let a = f
        .resolve_gid("src/A", "g")
        .expect("resolve A")
        .expect("some");
    let a_triples: Vec<_> = f.triples_ids(a.gid).expect("triples A").collect();
    assert_eq!(a_triples.len(), 2);

    // src/B remains unchanged
    let b = f
        .resolve_gid("src/B", "g")
        .expect("resolve B")
        .expect("some");
    let b_triples: Vec<_> = f.triples_ids(b.gid).expect("triples B").collect();
    assert_eq!(b_triples.len(), 1);
    // Check subject string for B stayed the same
    let (s_b, _, _) = b_triples[0];
    assert_eq!(f.term_to_string(s_b).unwrap(), "ex:s2");

    let _ = std::fs::remove_file(&in_path);
    let _ = std::fs::remove_file(&out_path);
}
