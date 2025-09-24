use rdf5d::{Quint, StreamingWriter, Term, reader::R5tuFile, writer::WriterOptions};

#[test]
fn streaming_writer_roundtrip_interleaved_order() {
    let mut path = std::env::temp_dir();
    path.push("stream_roundtrip.r5tu");
    let opts = WriterOptions {
        zstd: false,
        with_crc: true,
    };
    let mut w = StreamingWriter::new(&path, opts);

    // Intentionally interleave graphs and out-of-order SPO to ensure sorting at finalize
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

    w.add(Quint {
        id: "src/B".into(),
        s: s2.clone(),
        p: p1.clone(),
        o: o3.clone(),
        gname: "g".into(),
    })
    .unwrap();
    w.add(Quint {
        id: "src/A".into(),
        s: s1.clone(),
        p: p2.clone(),
        o: o2.clone(),
        gname: "g".into(),
    })
    .unwrap();
    w.add(Quint {
        id: "src/A".into(),
        s: s1.clone(),
        p: p1.clone(),
        o: o1.clone(),
        gname: "g".into(),
    })
    .unwrap();

    w.finalize().expect("finalize");

    let f = R5tuFile::open(&path).expect("open");
    let v = f.enumerate_by_graphname("g").unwrap();
    assert_eq!(v.len(), 2);
    // Graph A: verify both triples present via strings (order-agnostic)
    let a = f.resolve_gid("src/A", "g").unwrap().unwrap();
    let triples_a: Vec<_> = f.triples_ids(a.gid).unwrap().collect();
    assert_eq!(triples_a.len(), 2);
    let mut set_a = std::collections::HashSet::new();
    for (s, p, o) in triples_a {
        set_a.insert((
            f.term_to_string(s).unwrap(),
            f.term_to_string(p).unwrap(),
            f.term_to_string(o).unwrap(),
        ));
    }
    let mut expected_a = std::collections::HashSet::new();
    expected_a.insert((
        "http://ex/s1".to_string(),
        "http://ex/p1".to_string(),
        "\"v1\"".to_string(),
    ));
    expected_a.insert((
        "http://ex/s1".to_string(),
        "http://ex/p2".to_string(),
        "\"v2\"@en".to_string(),
    ));
    assert_eq!(set_a, expected_a);
    // Graph B: verify single triple
    let b = f.resolve_gid("src/B", "g").unwrap().unwrap();
    let triples_b: Vec<_> = f.triples_ids(b.gid).unwrap().collect();
    assert_eq!(triples_b.len(), 1);
    let (s, p, o) = triples_b[0];
    assert_eq!(f.term_to_string(s).unwrap(), "http://ex/s2");
    assert_eq!(f.term_to_string(p).unwrap(), "http://ex/p1");
    assert_eq!(f.term_to_string(o).unwrap(), "_:b3");

    let _ = std::fs::remove_file(&path);
}
