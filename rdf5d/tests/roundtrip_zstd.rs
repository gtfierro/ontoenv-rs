#![cfg(feature = "zstd")]
use rdf5d::{
    reader::R5tuFile,
    writer::{Quint, Term, WriterOptions, write_file_with_options},
};

#[test]
fn roundtrip_with_zstd_blocks_and_crc() {
    // Build input quints for two graphs under the same graphname
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

    let mut quints = Vec::new();
    quints.push(Quint {
        id: "src/A".into(),
        s: s1.clone(),
        p: p1.clone(),
        o: o1.clone(),
        gname: "g".into(),
    });
    quints.push(Quint {
        id: "src/A".into(),
        s: s1.clone(),
        p: p2.clone(),
        o: o2.clone(),
        gname: "g".into(),
    });
    quints.push(Quint {
        id: "src/B".into(),
        s: s2.clone(),
        p: p1.clone(),
        o: o3.clone(),
        gname: "g".into(),
    });

    let opts = WriterOptions {
        zstd: true,
        with_crc: true,
    };
    let mut path = std::env::temp_dir();
    path.push("roundtrip_zstd.r5tu");
    write_file_with_options(&path, &quints, opts).expect("write zstd file");

    let f = R5tuFile::open(&path).expect("open");
    // flags bit1 should be set (zstd)
    assert_eq!(f.header().flags_u16 & (1 << 1), 1 << 1);

    // enumerate_by_id("src/A") → 1 graph
    let v = f.enumerate_by_id("src/A").expect("enum id");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].n_triples, 2);

    // enumerate_by_graphname("g") → 2 graphs
    let w = f.enumerate_by_graphname("g").expect("enum g");
    assert_eq!(w.len(), 2);

    // triples for src/B / g
    let gr = f.resolve_gid("src/B", "g").expect("resolve").expect("some");
    let triples: Vec<_> = f.triples_ids(gr.gid).expect("triples").collect();
    assert_eq!(triples.len(), 1);
    let (s, p, o) = triples[0];
    assert_eq!(f.term_to_string(s).unwrap(), "http://ex/s2");
    assert_eq!(f.term_to_string(p).unwrap(), "http://ex/p1");
    assert_eq!(f.term_to_string(o).unwrap(), "_:b3");

    let _ = std::fs::remove_file(&path);
}
