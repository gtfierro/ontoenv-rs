use rdf5d::{
    StreamingWriter,
    reader::R5tuFile,
    writer::{Quint, Term, WriterOptions, write_file, write_file_with_options},
};

fn mk_temp(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(name);
    p
}

#[test]
fn empty_input_produces_valid_file() {
    let path = mk_temp("empty.r5tu");
    let quints: Vec<Quint> = Vec::new();
    write_file(&path, &quints).expect("write empty");
    let f = R5tuFile::open(&path).expect("open");
    assert!(!f.toc().is_empty()); // has sections
    // Enumerations yield empty
    assert!(f.enumerate_by_graphname("g").unwrap().is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn streaming_empty_finalize() {
    let path = mk_temp("empty_stream.r5tu");
    let w = StreamingWriter::new(
        &path,
        WriterOptions {
            zstd: false,
            with_crc: true,
        },
    );
    w.finalize().expect("finalize empty");
    let f = R5tuFile::open(&path).expect("open");
    assert!(!f.toc().is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn postings_monotonicity_and_spo_order() {
    // Build three graphs under two ids to exercise postings
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
        lang: None,
    };
    let o3 = Term::Literal {
        lex: "v3".into(),
        dt: None,
        lang: None,
    };
    let qs = vec![
        Quint {
            id: "A".into(),
            s: s1.clone(),
            p: p1.clone(),
            o: o1.clone(),
            gname: "g".into(),
        },
        Quint {
            id: "A".into(),
            s: s1.clone(),
            p: p2.clone(),
            o: o2.clone(),
            gname: "g".into(),
        },
        Quint {
            id: "B".into(),
            s: s2.clone(),
            p: p1.clone(),
            o: o3.clone(),
            gname: "g".into(),
        },
    ];
    let path = mk_temp("mono.r5tu");
    write_file_with_options(
        &path,
        &qs,
        WriterOptions {
            zstd: false,
            with_crc: true,
        },
    )
    .unwrap();
    let f = R5tuFile::open(&path).unwrap();
    // Postings monotonicity via enumerate_by_graphname("g"): gids must strictly increase
    let mut last_gid = None;
    for gr in f.enumerate_by_graphname("g").unwrap() {
        if let Some(g) = last_gid {
            assert!(gr.gid > g);
        }
        last_gid = Some(gr.gid);
        // Check SPO order non-decreasing within block and counts match
        let mut prev = None;
        let mut count = 0u64;
        for t in f.triples_ids(gr.gid).unwrap() {
            if let Some(pp) = prev {
                assert!(pp <= t);
            }
            prev = Some(t);
            count += 1;
        }
        assert_eq!(count, gr.n_triples);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn long_strings_and_lookup() {
    let long = "a".repeat(128);
    let q = Quint {
        id: long.clone(),
        s: Term::Iri("http://ex/s".into()),
        p: Term::Iri("http://ex/p".into()),
        o: Term::Iri("http://ex/o".into()),
        gname: long.clone(),
    };
    let path = mk_temp("longstrs.r5tu");
    write_file(&path, &[q]).unwrap();
    let f = R5tuFile::open(&path).unwrap();
    let by_id = f.enumerate_by_id(&long).unwrap();
    assert_eq!(by_id.len(), 1);
    let by_g = f.enumerate_by_graphname(&long).unwrap();
    assert_eq!(by_g.len(), 1);
    let _ = std::fs::remove_file(&path);
}
