# Remaining Performance Issues

## 1. `Rdf5dSnapshot::open()` still scales with total dataset size

The zero-copy `rdf5d` path avoids materializing the full dataset into Oxigraph, but snapshot creation is still doing a full triple scan up front.

Current behavior in [python/src/lib.rs](/Users/gabe/src/ontoenv-rs/python/src/lib.rs):

- `Rdf5dSnapshot::open()` opens `store.r5tu` with `R5tuFile::open_mmap(...)`
- it calls `enumerate_all()` to group physical gids by graph name
- for every logical graph, it calls `count_unique_triples_for_gids(...)`
- `count_unique_triples_for_gids(...)` scans all triples in the grouped gids and inserts `(s_id, p_id, o_id)` into a `HashSet` to precompute `triple_count`

Why this matters:

- dataset open latency is still proportional to total triples in the file
- large stores pay a full-store scan before the first query
- this undercuts the intended "fast open, explicit refresh" snapshot model

Related issue:

- `find_term_id()` still calls `find_decoded_term(...)`, so reverse term lookup is also not using the planned indexed path yet

Likely fix direction:

- stop eagerly computing per-logical-graph unique triple counts during snapshot open
- either compute counts lazily on first `len(...)` access or store the needed aggregate/index data in `rdf5d`
- add the planned reverse lookup index so `find_term_id()` is no longer a linear scan

## 2. Copy fallback still does two Python-side materialization passes

The fallback path for temporary envs and external `graph_store=` envs is still heavier than it needs to be.

Current behavior in [python/ontoenv/rdflib_store.py](/Users/gabe/src/ontoenv-rs/python/ontoenv/rdflib_store.py) and [python/src/lib.rs](/Users/gabe/src/ontoenv-rs/python/src/lib.rs):

- `_copy_env_into_store()` builds an rdflib `Dataset`
- it copies every env graph into that rdflib dataset
- it then iterates every quad back out of rdflib
- `bind_materialized_snapshot(...)` converts all of those Python terms back into Rust/Oxigraph terms and inserts them into an `OxDataset`

Why this matters:

- the fallback path does two full passes through Python objects
- every quad is allocated and decoded twice on the Python side
- the final Rust snapshot is correct, but the path is much more expensive than necessary
- `mode="auto"` hits this path for temporary envs and external-store-backed envs, so this is not just an obscure edge case

Likely fix direction:

- build the materialized fallback snapshot directly from the env/store into Rust, without round-tripping through an rdflib `Dataset`
- keep rdflib as the consumer-facing API, not the staging area for snapshot construction
- if Python must stay involved, pass a simpler quad stream once rather than materializing into rdflib and then re-reading it
