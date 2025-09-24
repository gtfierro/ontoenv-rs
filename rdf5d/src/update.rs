//! Update helpers for replacing an entire logical graph in an existing file.
//!
//! The primary entry is [`replace_graph_with_options`], which reads an input
//! `.r5tu` file and writes a new file where the target (id, graphname) group is
//! fully replaced with provided triples, preserving all other groups.
//!
//! This performs a streaming rebuild using [`StreamingWriter`], avoiding
//! materializing the entire dataset in memory. It reconstructs writer terms from
//! the original file's term dictionary to faithfully copy unchanged graphs.
//!
//! Basic example
//!
//! ```no_run
//! use rdf5d::{replace_graph, Term};
//! // New triples for the target graph
//! let new_triples = vec![
//!     (
//!         Term::Iri("http://ex/s1".into()),
//!         Term::Iri("http://ex/p".into()),
//!         Term::Literal { lex: "v2".into(), dt: None, lang: None },
//!     )
//! ];
//! replace_graph(
//!     "in.r5tu",
//!     "out.r5tu",
//!     "src/A",
//!     "http://example.org/graph",
//!     &new_triples,
//! ).expect("update ok");
//! ```

use std::path::Path;

use crate::reader::{GraphRef, R5tuFile, Result};
use crate::writer::{Quint, StreamingWriter, Term, WriterOptions};

fn copy_group_as_quints(file: &R5tuFile, gr: &GraphRef) -> Result<Vec<Quint>> {
    let mut out = Vec::with_capacity(gr.n_triples as usize);
    let iter = file.triples_ids(gr.gid)?;
    for (s_id, p_id, o_id) in iter {
        let s = file.term_as_writer_term(s_id)?;
        let p = file.term_as_writer_term(p_id)?;
        let o = file.term_as_writer_term(o_id)?;
        out.push(Quint {
            id: gr.id.clone(),
            s,
            p,
            o,
            gname: gr.graphname.clone(),
        });
    }
    Ok(out)
}

/// Replace one logical graph (matching `id` and `gname`) and write a new file.
///
/// - Preserves all other graphs as-is.
/// - Rebuilds the file using [`StreamingWriter`] for determinism and integrity.
/// - Uses default writer options: no zstd, CRCs enabled.
pub fn replace_graph<P: AsRef<Path>>(
    src: P,
    dst: P,
    id: &str,
    gname: &str,
    new_triples: &[(Term, Term, Term)],
) -> Result<()> {
    replace_graph_with_options(
        src,
        dst,
        id,
        gname,
        new_triples,
        WriterOptions {
            zstd: false,
            with_crc: true,
        },
    )
}

/// Replace one logical graph (matching `id` and `gname`) and write a new file.
///
/// - Preserves all other graphs as-is by reconstructing their triples from the
///   original term dictionary (no string lossy conversion).
/// - Rebuilds the file using [`StreamingWriter`] and given [`WriterOptions`].
/// - The `new_triples` iterator yields `(s, p, o)` terms; the `id` and
///   `gname` are applied to every triple.
///
/// Errors surface from input validation, decoding, or I/O per [`R5tuFile`].
pub fn replace_graph_with_options<P: AsRef<Path>>(
    src: P,
    dst: P,
    id: &str,
    gname: &str,
    new_triples: &[(Term, Term, Term)],
    opts: WriterOptions,
) -> Result<()> {
    let f = R5tuFile::open(src.as_ref())?;
    let mut w = StreamingWriter::new(dst.as_ref().to_path_buf(), opts);

    // 1) Copy all existing groups except the target
    for gr in f.enumerate_all()? {
        if gr.id == id && gr.graphname == gname {
            continue;
        }
        for q in copy_group_as_quints(&f, &gr)? {
            w.add(q)?;
        }
    }

    // 2) Insert replacement graph
    for (s, p, o) in new_triples.iter().cloned() {
        w.add(Quint {
            id: id.to_string(),
            s,
            p,
            o,
            gname: gname.to_string(),
        })?;
    }

    // 3) Finalize (atomic write)
    w.finalize()
}
