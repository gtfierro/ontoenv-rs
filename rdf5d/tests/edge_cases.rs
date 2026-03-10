use rdf5d::{
    StreamingWriter,
    header::SectionKind,
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

#[test]
fn term_dict_uses_u32_offsets_when_possible() {
    let q = Quint {
        id: "id".into(),
        s: Term::Iri("http://ex/s".into()),
        p: Term::Iri("http://ex/p".into()),
        o: Term::Literal {
            lex: "v".into(),
            dt: None,
            lang: None,
        },
        gname: "g".into(),
    };
    let path = mk_temp("term_width4.r5tu");
    write_file(&path, &[q]).unwrap();
    let f = R5tuFile::open(&path).unwrap();
    let sec = f.section(SectionKind::TermDict).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes[sec.off as usize], 4);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn string_dict_uses_compact_index_stride() {
    let q = Quint {
        id: "dataset/alpha".into(),
        s: Term::Iri("http://ex/s".into()),
        p: Term::Iri("http://ex/p".into()),
        o: Term::Iri("http://ex/o".into()),
        gname: "http://example.org/graph/alpha".into(),
    };
    let path = mk_temp("dict_stride20.r5tu");
    write_file(&path, &[q]).unwrap();
    let f = R5tuFile::open(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    for kind in [SectionKind::IdDict, SectionKind::GNameDict] {
        let sec = f.section(kind).unwrap();
        let base = sec.off as usize;
        let n = u32::from_le_bytes(bytes[base..base + 4].try_into().unwrap()) as usize;
        let idx_len = u64::from_le_bytes(bytes[base + 44..base + 52].try_into().unwrap()) as usize;
        assert_eq!(idx_len / n, 20);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn gdir_and_pair_index_use_compact_layouts_when_possible() {
    let q1 = Quint {
        id: "dataset/a".into(),
        s: Term::Iri("http://ex/s1".into()),
        p: Term::Iri("http://ex/p".into()),
        o: Term::Iri("http://ex/o1".into()),
        gname: "g".into(),
    };
    let q2 = Quint {
        id: "dataset/b".into(),
        s: Term::Iri("http://ex/s2".into()),
        p: Term::Iri("http://ex/p".into()),
        o: Term::Iri("http://ex/o2".into()),
        gname: "g".into(),
    };
    let path = mk_temp("compact_meta.r5tu");
    write_file(&path, &[q1, q2]).unwrap();
    let f = R5tuFile::open(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();

    let gdir = f.section(SectionKind::GDir).unwrap();
    let gdir_base = gdir.off as usize;
    let row_size = u32::from_le_bytes(bytes[gdir_base + 8..gdir_base + 12].try_into().unwrap());
    assert_eq!(row_size, 32);

    let pair = f.section(SectionKind::IdxPair2Gid).unwrap();
    let pair_base = pair.off as usize;
    let n_pairs = u64::from_le_bytes(bytes[pair_base..pair_base + 8].try_into().unwrap()) as usize;
    let pairs_off =
        u64::from_le_bytes(bytes[pair_base + 8..pair_base + 16].try_into().unwrap()) as usize;
    let pair_stride = (pair.len as usize - (pairs_off - pair_base)) / n_pairs;
    assert_eq!(pair_stride, 12);

    let _ = std::fs::remove_file(&path);
}
