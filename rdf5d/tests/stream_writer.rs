use rdf5d::{
    Quint, SpillPolicy, StreamingWriter, StreamingWriterOptions, Term, reader::R5tuFile,
    writer::WriterOptions,
};

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

#[test]
fn streaming_writer_spills_sorted_runs_and_merges() {
    let mut path = std::env::temp_dir();
    path.push("stream_runs_roundtrip.r5tu");
    let opts = WriterOptions {
        zstd: false,
        with_crc: true,
    };
    let mut w = StreamingWriter::with_chunk_capacity(&path, opts, 0, 2);

    for i in (0..8).rev() {
        w.add(Quint {
            id: if i % 2 == 0 { "src/A" } else { "src/B" }.into(),
            s: Term::Iri(format!("http://ex/s{i}")),
            p: Term::Iri("http://ex/p".into()),
            o: Term::Iri(format!("http://ex/o{i}")),
            gname: "g".into(),
        })
        .unwrap();
    }

    let stats = w.finalize_with_stats().expect("finalize");
    assert_eq!(stats.total_quads, 8);
    assert_eq!(stats.chunk_quads, 2);
    assert_eq!(stats.max_pending_quads, 2);
    assert_eq!(stats.run_count, 4);
    assert_eq!(stats.temp_bytes_written, 256);

    let f = R5tuFile::open(&path).expect("open");
    let graphs = f.enumerate_by_graphname("g").unwrap();
    assert_eq!(graphs.len(), 2);
    for gr in graphs {
        let triples: Vec<_> = f.triples_ids(gr.gid).unwrap().collect();
        assert_eq!(triples.len(), 4);
        let mut prev = None;
        for t in triples {
            if let Some(p) = prev {
                assert!(p <= t);
            }
            prev = Some(t);
        }
    }

    let _ = std::fs::remove_file(&path);
}

#[test]
fn streaming_writer_reports_zero_temp_usage_without_spills() {
    let mut path = std::env::temp_dir();
    path.push("stream_no_spill_stats.r5tu");
    let opts = WriterOptions {
        zstd: false,
        with_crc: true,
    };
    let mut w = StreamingWriter::with_chunk_capacity(&path, opts, 0, 16);

    for i in 0..3 {
        w.add(Quint {
            id: "src/A".into(),
            s: Term::Iri(format!("http://ex/s{i}")),
            p: Term::Iri("http://ex/p".into()),
            o: Term::Iri(format!("http://ex/o{i}")),
            gname: "g".into(),
        })
        .unwrap();
    }

    let stats = w.finalize_with_stats().expect("finalize");
    assert_eq!(stats.total_quads, 3);
    assert_eq!(stats.chunk_quads, 16);
    assert_eq!(stats.max_pending_quads, 3);
    assert_eq!(stats.run_count, 0);
    assert_eq!(stats.temp_bytes_written, 0);

    let f = R5tuFile::open(&path).expect("open");
    let gr = f.resolve_gid("src/A", "g").unwrap().unwrap();
    let triples: Vec<_> = f.triples_ids(gr.gid).unwrap().collect();
    assert_eq!(triples.len(), 3);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn streaming_writer_auto_policy_uses_workload_hint_without_spilling_small_inputs() {
    let mut path = std::env::temp_dir();
    path.push("stream_auto_policy.r5tu");
    let mut w = StreamingWriter::with_options_and_hint(
        &path,
        StreamingWriterOptions {
            writer: WriterOptions {
                zstd: false,
                with_crc: true,
            },
            spill_policy: SpillPolicy::Auto,
        },
        3,
    );

    for i in 0..3 {
        w.add(Quint {
            id: "src/A".into(),
            s: Term::Iri(format!("http://ex/s{i}")),
            p: Term::Iri("http://ex/p".into()),
            o: Term::Iri(format!("http://ex/o{i}")),
            gname: "g".into(),
        })
        .unwrap();
    }

    let stats = w.finalize_with_stats().expect("finalize");
    assert_eq!(stats.chunk_quads, 1024);
    assert_eq!(stats.run_count, 0);
    assert_eq!(stats.temp_bytes_written, 0);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn streaming_writer_target_pending_bytes_maps_to_quad_threshold() {
    let mut path = std::env::temp_dir();
    path.push("stream_byte_policy.r5tu");
    let mut w = StreamingWriter::with_options(
        &path,
        StreamingWriterOptions {
            writer: WriterOptions {
                zstd: false,
                with_crc: true,
            },
            spill_policy: SpillPolicy::TargetPendingBytes(64),
        },
    );

    for i in 0..3 {
        w.add(Quint {
            id: "src/A".into(),
            s: Term::Iri(format!("http://ex/s{i}")),
            p: Term::Iri("http://ex/p".into()),
            o: Term::Iri(format!("http://ex/o{i}")),
            gname: "g".into(),
        })
        .unwrap();
    }

    let stats = w.finalize_with_stats().expect("finalize");
    assert_eq!(stats.chunk_quads, 2);
    assert_eq!(stats.run_count, 2);
    assert_eq!(stats.temp_bytes_written, 96);

    let _ = std::fs::remove_file(&path);
}
