# Remaining Performance Tradeoffs

## 1. Binding an rdf5d snapshot eagerly builds permutation indexes

`Snapshot::open()` now groups physical graph IDs without precomputing unique
triple counts, and reverse term lookup uses an in-memory term index. Binding
the snapshot to the Python read backend deliberately calls `build_indexes()`,
which builds the PSO, POS, SPO, and OSP indexes in parallel.

Consequences:

- first `get_dataset()`, `get_graph()`, or `get_closure()` access scales with
  the total snapshot size;
- the four indexes use several times the on-disk snapshot size in RAM;
- later bound-term scans and SPARQL queries avoid full graph scans.

This is currently an intentional cold-start versus steady-state-query
tradeoff. Benchmarks should report view construction separately from warmed
query execution.

The property-path closure index remains lazy and is built on the first
supported recursive path query.

## 2. Temporary and custom-store closure views require materialization

Persistent local environments can apply closure normalization as a small patch
over the mmap snapshot. Temporary environments and external `graph_store=`
backends have no rdf5d term-ID space, so `get_closure()` instead builds a
dedicated normalized in-memory `OxDataset`.

The fallback now:

- reads each graph through the backend's `get_graph` path;
- performs the import, ontology-declaration, and SHACL-prefix transforms in
  Rust;
- stores the flattened result once as a read-only snapshot;
- returns the same triple set as `copy_closure()`.

It no longer stages through an rdflib `Dataset` and then reads every Python
quad back into Rust. It still necessarily materializes the requested closure,
so callers that need true zero-copy traversal should use a persistent local
rdf5d-backed environment.
