#![cfg(feature = "mmap")]
use rdf5d::{Quint, R5tuFile, StreamingWriter, Term, writer::WriterOptions};

#[test]
fn mmap_roundtrip_two_graphs() {
    // Build two graphs under same graphname, different ids
    let mut quints = Vec::new();
    // Graph A: two triples
    quints.push(Quint {
        id: "src/A".into(),
        gname: "g".into(),
        s: Term::Iri("http://ex/s1".into()),
        p: Term::Iri("http://ex/p1".into()),
        o: Term::Iri("http://ex/o1".into()),
    });
    quints.push(Quint {
        id: "src/A".into(),
        gname: "g".into(),
        s: Term::Iri("http://ex/s1".into()),
        p: Term::Iri("http://ex/p2".into()),
        o: Term::Literal {
            lex: "v2".into(),
            dt: None,
            lang: Some("en".into()),
        },
    });
    // Graph B: one triple
    quints.push(Quint {
        id: "src/B".into(),
        gname: "g".into(),
        s: Term::Iri("http://ex/s2".into()),
        p: Term::Iri("http://ex/p1".into()),
        o: Term::Literal {
            lex: "42".into(),
            dt: Some("http://www.w3.org/2001/XMLSchema#integer".into()),
            lang: None,
        },
    });

    // Write file
    let mut path = std::env::temp_dir();
    path.push("e2e_mmap.r5tu");
    let opts = WriterOptions {
        zstd: false,
        with_crc: true,
    };
    let mut w = StreamingWriter::new(&path, opts);
    for q in quints {
        w.add(q).unwrap();
    }
    w.finalize().unwrap();

    // Open via mmap
    let f = R5tuFile::open_mmap(&path).expect("open_mmap");

    // enumerate_by_id
    let a = f.enumerate_by_id("src/A").unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].n_triples, 2);
    // enumerate_by_graphname
    let g = f.enumerate_by_graphname("g").unwrap();
    assert_eq!(g.len(), 2);
    // resolve_gid + iterate
    let gr_b = f.resolve_gid("src/B", "g").unwrap().unwrap();
    let ts_b: Vec<_> = f.triples_ids(gr_b.gid).unwrap().collect();
    assert_eq!(ts_b.len(), 1);
    let (s, p, o) = ts_b[0];
    assert_eq!(f.term_to_string(s).unwrap(), "http://ex/s2");
    assert_eq!(f.term_to_string(p).unwrap(), "http://ex/p1");
    assert_eq!(
        f.term_to_string(o).unwrap(),
        "\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>"
    );

    let _ = std::fs::remove_file(&path);
}
