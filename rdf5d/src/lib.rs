//! rdf5d — Compact, mmap‑friendly storage for RDF 5‑tuples (R5TU).
//!
//! This crate provides a tiny reader and writer for the on‑disk format
//! described in ARCH.md. It focuses on fast, bounded reads suitable for
//! memory‑mapped access and simple, deterministic file production.
//!
//! Quick start: write a file
//!
//! ```no_run
//! use rdf5d::{write_file, Quint, Term};
//! use std::path::PathBuf;
//!
//! let path = PathBuf::from("example.r5tu");
//! let quads = vec![
//!     Quint {
//!         id: "dataset:1".into(),
//!         s: Term::Iri("http://example.org/Alice".into()),
//!         p: Term::Iri("http://xmlns.com/foaf/0.1/name".into()),
//!         o: Term::Literal { lex: "Alice".into(), dt: None, lang: None },
//!         gname: "http://example.org/graph".into(),
//!     },
//! ];
//!
//! write_file(&path, &quads).expect("write ok");
//! ```
//!
//! Read it back and enumerate graph groups
//!
//! ```no_run
//! use rdf5d::R5tuFile;
//! use std::path::Path;
//!
//! let f = R5tuFile::open(Path::new("example.r5tu")).expect("open");
//! // List groups by a dataset id
//! let hits = f.enumerate_by_id("dataset:1").expect("lookup");
//! for g in hits {
//!     println!("gid={} graph={} triples={} id={}", g.gid, g.graphname, g.n_triples, g.id);
//! }
//! ```
//!
//! See `ARCH.md` for details on the layout and terminology.

pub mod header;
pub mod reader;
pub mod update;
pub mod writer;

pub use reader::{GraphRef, R5tuFile};
pub use update::{replace_graph, replace_graph_with_options};
pub use writer::{
    Quint, StreamingWriter, Term, WriterOptions, write_file, write_file_with_options,
};

/// Crate‑level result type using the reader error.
pub type Result<T> = std::result::Result<T, crate::reader::R5Error>;
