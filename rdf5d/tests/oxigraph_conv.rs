#![cfg(feature = "oxigraph")]
use rdf5d::{
    reader::R5tuFile,
    writer::{Quint, Term, write_file},
};

#[test]
fn to_oxigraph_graph_basic() {
    let s1 = Term::Iri("http://ex/s1".into());
    let p1 = Term::Iri("http://ex/p1".into());
    let o1 = Term::Literal {
        lex: "v1".into(),
        dt: None,
        lang: Some("en".into()),
    };
    let q = Quint {
        id: "src/A".into(),
        s: s1,
        p: p1,
        o: o1,
        gname: "g".into(),
    };
    let mut path = std::env::temp_dir();
    path.push("oxigraph_conv.r5tu");
    write_file(&path, &[q]).unwrap();
    let f = R5tuFile::open(&path).unwrap();
    let gr = f.resolve_gid("src/A", "g").unwrap().unwrap();
    let g = f.to_oxigraph_graph(gr.gid).unwrap();
    assert_eq!(g.iter().count(), 1);
    // Iterator over oxigraph triples
    let triples: Vec<_> = f
        .oxigraph_triples(gr.gid)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(triples.len(), 1);
    let _ = std::fs::remove_file(&path);
}
