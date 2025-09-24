use rdf5d::{
    reader::R5tuFile,
    writer::{Quint, Term, write_file},
};

#[test]
fn writer_reader_roundtrip_two_graphs() {
    // Build input quints (ids: src/A, src/B; graphname: g)
    // Graph A: (s1,p1,o1), (s1,p2,o2)
    // Graph B: (s2,p1,o3)
    let s1 = Term::Iri("http://ex/s1".into());
    let s2 = Term::Iri("http://ex/s2".into());
    let p1 = Term::Iri("http://ex/p1".into());
    let p2 = Term::Iri("http://ex/p2".into());
    let o1 = Term::Literal {
        lex: "v1".into(),
        dt: None,
        lang: None,
    };
    let o2 = Term::Literal {
        lex: "v2".into(),
        dt: None,
        lang: Some("en".into()),
    };
    let o3 = Term::BNode("_:b3".into());

    let quints = vec![
        Quint {
            id: "src/A".into(),
            s: s1.clone(),
            p: p1.clone(),
            o: o1.clone(),
            gname: "g".into(),
        },
        Quint {
            id: "src/A".into(),
            s: s1.clone(),
            p: p2.clone(),
            o: o2.clone(),
            gname: "g".into(),
        },
        Quint {
            id: "src/B".into(),
            s: s2.clone(),
            p: p1.clone(),
            o: o3.clone(),
            gname: "g".into(),
        },
    ];

    let mut path = std::env::temp_dir();
    path.push("roundtrip.r5tu");
    write_file(&path, &quints).expect("write");

    let f = R5tuFile::open(&path).expect("open");

    // enumerate_by_id("src/A")
    let v = f.enumerate_by_id("src/A").expect("enum id");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].n_triples, 2);

    // enumerate_by_graphname("g") => 2 graphs
    let w = f.enumerate_by_graphname("g").expect("enum g");
    assert_eq!(w.len(), 2);
    assert!(w.iter().any(|gr| gr.id == "src/A"));
    assert!(w.iter().any(|gr| gr.id == "src/B"));

    // resolve_gid("src/B","g")
    let gr = f.resolve_gid("src/B", "g").expect("resolve").expect("some");
    let triples: Vec<_> = f.triples_ids(gr.gid).expect("triples").collect();
    assert_eq!(triples.len(), 1);
    let (s, p, o) = triples[0];
    // term_to_string reproduces
    let ss = f.term_to_string(s).expect("s");
    let pp = f.term_to_string(p).expect("p");
    let oo = f.term_to_string(o).expect("o");
    assert_eq!(ss, "http://ex/s2");
    assert_eq!(pp, "http://ex/p1");
    assert_eq!(oo, "_:b3");

    let _ = std::fs::remove_file(&path);
}
