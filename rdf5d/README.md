# rdf5d (R5TU) — Rust Library and CLI

R5TU is a compact, mmap‑friendly on‑disk format for RDF 5‑tuples. This crate provides:
- A zero‑copy reader API to enumerate graphs and stream triples.
- A writer and streaming writer to build files from your data or Oxigraph graphs.
- An optional CLI (feature `oxigraph`) for quick imports and sanity checks.

See `ARCH.md` for format details.

## Add to your project

In your `Cargo.toml`:

```
[dependencies]
rdf5d = { path = "." }
# optional
# rdf5d = { version = "0.1", features = ["oxigraph", "zstd"] }
```

Features:
- `zstd`: enable zstd‑compressed triple blocks.
- `oxigraph`: integrate with `oxigraph` model + parsers and build the CLI.

## Reading an .r5tu file

```rust
use rdf5d::R5tuFile;
use std::path::Path;

let f = R5tuFile::open(Path::new("data.r5tu"))?;

// Enumerate by source id
for gr in f.enumerate_by_id("src/A")? {
    println!("gid={} id={} gname={} n={}", gr.gid, gr.id, gr.graphname, gr.n_triples);
}

// Resolve a specific (id, graphname)
if let Some(gr) = f.resolve_gid("src/A", "g")? {
    // Stream triples as TermIDs and render to strings on demand
    for (s, p, o) in f.triples_ids(gr.gid)? {
        println!("{} {} {}",
            f.term_to_string(s)?,
            f.term_to_string(p)?,
            f.term_to_string(o)?,
        );
    }
}
```

With feature `oxigraph`, convert to Oxigraph types:

```rust
#[cfg(feature = "oxigraph")]
{
    let gr = f.resolve_gid("src/A","g")?.unwrap();
    let g = f.to_oxigraph_graph(gr.gid)?; // materialized Graph
    for t in f.oxigraph_triples(gr.gid)? { // streaming iterator
        let t = t?; /* use t: oxigraph::model::Triple */
    }
}
```

## Writing files

Simple batch write:

```rust
use rdf5d::{writer::write_file_with_options, writer::{WriterOptions}, Quint, Term};

let quints = vec![
    Quint{ id:"src/A".into(), gname:"g".into(),
           s: Term::Iri("http://ex/s".into()),
           p: Term::Iri("http://ex/p".into()),
           o: Term::Literal{ lex:"v".into(), dt: None, lang: None }},
];

write_file_with_options("out.r5tu", &quints, WriterOptions{ zstd: false, with_crc: true })?;
```

Streaming writer (append quads incrementally):

```rust
use rdf5d::{StreamingWriter, Term, Quint};
let mut w = StreamingWriter::new("out.r5tu", rdf5d::writer::WriterOptions{ zstd:false, with_crc:true });
w.add(Quint{ id:"src/A".into(), gname:"g".into(),
             s: Term::Iri("http://ex/s1".into()),
             p: Term::Iri("http://ex/p1".into()),
             o: Term::Iri("http://ex/o1".into()) })?;
w.finalize()?;
```

With feature `oxigraph`, write from an Oxigraph Graph:

```rust
#[cfg(feature = "oxigraph")]
{
    use rdf5d::writer::{write_graph_from_oxigraph, WriterOptions};
    let graph = oxigraph::model::Graph::new();
    write_graph_from_oxigraph("out.r5tu", &graph, "src/A", "g",
        WriterOptions{ zstd:true, with_crc:true })?;
}
```

## CLI (feature `oxigraph`)

Build: `cargo build --features oxigraph --bin r5tu`

Examples:
- Import graphs (multiple inputs) to one file:
  - `r5tu build-graph --input a.ttl --input b.nt --output out.r5tu --graphname g`
- Import dataset (TriG/NQuads):
  - `r5tu build-dataset --input data.trig --output out.r5tu --default-graphname default`
- Basic stats:
  - `r5tu stat --file out.r5tu`

```text
Flags:
  --zstd         compress triple blocks
  --no-crc       skip writing per-section/global CRCs
```

## Notes
- Reader returns empty lists for unknown ids/graphnames.
- CRCs and footer are verified during open when present.
- Sections are validated for bounds and overlap.
