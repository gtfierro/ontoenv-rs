use rdf5d::{
    reader::{IntegrityMode, OpenOptions, R5tuFile},
    writer::{Quint, Term, write_file},
};

#[test]
fn detects_global_crc_mismatch() {
    let q = Quint {
        id: "X".into(),
        s: Term::Iri("http://ex/s".into()),
        p: Term::Iri("http://ex/p".into()),
        o: Term::Literal {
            lex: "v".into(),
            dt: None,
            lang: None,
        },
        gname: "g".into(),
    };
    let mut path = std::env::temp_dir();
    path.push("crc_bad.r5tu");
    write_file(&path, &[q]).unwrap();

    // Corrupt a byte in the middle of the file (but not the header magic)
    let mut bytes = std::fs::read(&path).unwrap();
    let pos = 40.min(bytes.len() - 17); // before footer
    bytes[pos] ^= 0xFF; // flip
    std::fs::write(&path, &bytes).unwrap();

    let err = R5tuFile::open(&path).unwrap_err();
    let _ = std::fs::remove_file(&path);
    match err {
        rdf5d::reader::R5Error::Corrupt(m) => assert!(m.contains("CRC")),
        _ => panic!("expected CRC mismatch error"),
    }
}

#[test]
fn structural_open_can_skip_crc_mismatch() {
    let q = Quint {
        id: "X".into(),
        s: Term::Iri("http://ex/s".into()),
        p: Term::Iri("http://ex/p".into()),
        o: Term::Literal {
            lex: "v".into(),
            dt: None,
            lang: None,
        },
        gname: "g".into(),
    };
    let mut path = std::env::temp_dir();
    path.push("crc_bad_structural.r5tu");
    write_file(&path, &[q]).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let footer_crc_pos = bytes.len() - 16;
    bytes[footer_crc_pos] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();

    let f = R5tuFile::open_with_options(
        &path,
        OpenOptions {
            integrity: IntegrityMode::Structural,
            prefer_mmap: false,
        },
    )
    .unwrap();
    assert_eq!(f.enumerate_by_id("X").unwrap().len(), 1);

    let _ = std::fs::remove_file(&path);
}
