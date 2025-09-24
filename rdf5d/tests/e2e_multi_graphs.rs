use rdf5d::{Quint, R5tuFile, StreamingWriter, Term, writer::WriterOptions};

#[test]
fn end_to_end_multiple_graphs_and_indexes() {
    // Build three graphs across two ids and two graphnames
    let qs = vec![
        Quint {
            id: "src/A".into(),
            gname: "g1".into(),
            s: Term::Iri("ex:s1".into()),
            p: Term::Iri("ex:p".into()),
            o: Term::Iri("ex:o1".into()),
        },
        Quint {
            id: "src/A".into(),
            gname: "g2".into(),
            s: Term::Iri("ex:s2".into()),
            p: Term::Iri("ex:p".into()),
            o: Term::Iri("ex:o2".into()),
        },
        Quint {
            id: "src/B".into(),
            gname: "g2".into(),
            s: Term::Iri("ex:s3".into()),
            p: Term::Iri("ex:p".into()),
            o: Term::Iri("ex:o3".into()),
        },
    ];

    let mut path = std::env::temp_dir();
    path.push("e2e_multi.r5tu");
    let mut w = StreamingWriter::new(
        &path,
        WriterOptions {
            zstd: false,
            with_crc: true,
        },
    );
    for q in qs {
        w.add(q).unwrap();
    }
    w.finalize().unwrap();

    let f = R5tuFile::open(&path).unwrap();
    // by id
    let a = f.enumerate_by_id("src/A").unwrap();
    assert_eq!(a.len(), 2);
    // by graphname
    let g2 = f.enumerate_by_graphname("g2").unwrap();
    assert_eq!(g2.len(), 2);
    // resolve pair
    let gr = f.resolve_gid("src/B", "g2").unwrap().unwrap();
    let v: Vec<_> = f.triples_ids(gr.gid).unwrap().collect();
    assert_eq!(v.len(), 1);
    let (s, _, _) = v[0];
    assert_eq!(f.term_to_string(s).unwrap(), "ex:s3");

    let _ = std::fs::remove_file(&path);
}
