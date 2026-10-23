# Experimental OntoEnv publication and snapshots

Publication coordinates an OntoEnv catalog, configuration and immutable RDF5D
artifact. RDF5D itself remains a file builder and query engine; it does not
implement environment transactions. The feature name remains
`transactional-snapshots` while this is experimental. See
[the boundary and manifest review](environment-publication.md) before treating
this prototype as a production storage design.

The `transactional-snapshots` feature adds an experimental Rust storage API:
`SnapshotWriter`, `SnapshotUpdate`, `SnapshotReader`, and `EnvironmentSnapshot`.
It requires Rust 1.90+ because it uses redb 4.3.0's experimental multiprocess
`SingleWriter` mode. Default-feature builds retain the existing compiler floor.

This is a separate prototype, not a migration of the CLI/Python API. It writes
`.ontoenv-snapshots/` and leaves an existing `.ontoenv/` alone.

## Try it

```sh
cargo run -p ontoenv --features transactional-snapshots --example transactional_snapshots
cargo test -p ontoenv --features transactional-snapshots --lib --test test_snapshots --test test_concurrency
```

The example keeps a writer open, opens a reader, publishes new graph content and
an alias, then refreshes the reader. A retained old query view still reads the
old generation.

```rust
let mut writer = SnapshotWriter::create(config)?;
let mut update = writer.begin_update();
let id = update.add_from_bytes(location, bytes, format)?;
update.add_alias("urn:alias", id.name().as_str())?;
let committed = update.commit()?;

let mut reader = SnapshotReader::open(root)?;
let old = reader.snapshot();
// Further writer commits do not change `old` or reader.snapshot().
reader.refresh()?;
```

Readers expose catalog listings, aliases, configuration, graph copies, generation
information and the existing RDF5D `Snapshot`/`View` engine. They have no mutation
or save methods. `committed_at` records the time while preparing publication;
generation numbers, not timestamps, order commits. `opened_at` records this
process's snapshot creation time.

## Commit protocol and copy-on-write

An update clones small catalog/configuration values and holds a graph overlay
of replacements and deletions. Untouched content remains in the immutable,
mmap-backed base. It does not clone the entire decoded store at transaction start.

On a graph-changing commit, it streams unchanged graphs and replacements into a
new complete RDF5D file. Complete files preserve the existing shared term
dictionary and cross-graph query engine. Empty replacements override old graph
data explicitly; empty graph existence is represented in the catalog.

The candidate file is synced and validated before one redb transaction publishes
its reference, metadata, aliases, configuration and next generation. The writer
then installs the already-prepared committed snapshot. A metadata-only update
reuses the same mapped file and performs no RDF5D rewrite.

Dropping or aborting an update publishes nothing. Strict commits reject unresolved
imports across the complete candidate. `add_source` and `add_from_bytes` currently
stage one ontology; stage its dependencies in the same transaction. Recursive
discovery/fetching and non-strict failure reports are not implemented yet.

The sole writer holds a stable `.ontoenv-snapshots.lock` outside the data directory.
Readers instead use redb read-only connections and short manifest transactions.
They retain immutable graph mappings and own no lifetime writer lock or database
read transaction. Initialization is prepared in a private directory and renamed
into place only after generation zero is complete. Recreation is deliberately
unsupported rather than deleting a live lock or mapped files.

## Retention and recovery limits

Published generations and complete unreferenced objects are **retained
indefinitely**. Automatic pruning, reset and migration are not exposed. Do not
delete old objects manually while any reader or query view can use them. This
initial policy avoids GC races and lets sleeping readers retain snapshots.

A crash before manifest publication leaves the previous generation selected;
it may leave an unreferenced file. A crash after publication selects the complete
new generation. A database commit error may have an uncertain durable outcome:
close/reopen the writer and inspect the committed generation before retrying.
Further commits on that writer are rejected; its last confirmed snapshot may
lag durable head. A known failure before attempting publication permits retry.
Opening a writer performs redb's required recovery after an abrupt writer exit;
fresh read-only connections are refused after an unclean writer exit until a
writer performs recovery. Already retained RDF5D views remain usable. This
availability limitation is one reason to evaluate immutable manifest files.

Writer recovery and reader/reader/writer coexistence have passed child-process
tests on Linux, macOS and Windows. This does not prove power-loss behavior on every filesystem
or portability to other operating systems. External mutation/truncation of an
immutable mapped object violates the storage contract; checksums do not make
such mutation safe.

## Measure before adding more complexity

```sh
cargo run --release -p ontoenv --features transactional-snapshots --example snapshot_profile -- 100 500
```

This diagnostic profile reports encoded size, serialization/publication time,
direct mmap versus owned-byte open time, manifest-plus-mmap open time,
one-graph update/full-file commit time and reader refresh time. Opens are repeated
against a warm page cache; this is not a cold-cache or statistically rigorous
benchmark. Existing strict CRC validation remains enabled, which can touch the
entire encoded file even with mmap. Triple decoding and query indexes remain
lazy.

Three sequential release-mode runs on October 23, 2026, with 100 graphs and
50,100 triples produced approximately 13.91 MiB RDF5D stores. They used rustc
1.99.1 on x86_64 Linux. The ranges below span the three runs, not confidence
intervals. Raw output, commands and source hashes are retained in
`docs/reviews/snapshot-profile-2026-10-23.txt` in the repository.

| Operation | Observed range |
| --- | --- |
| Initial source generation, parsing and staging | 129–133 ms |
| Initial serialization/publication (after source parsing) | 166–182 ms |
| Warm direct RDF5D mmap open | 11.8–12.7 ms |
| Warm direct RDF5D owned-byte open | 12.7–13.4 ms |
| Warm manifest plus RDF5D mmap open | 12.1–21.5 ms |
| One-graph source generation, parsing and staging | 1.0–2.2 ms |
| One graph changed, complete-file serialization/publication | 179–211 ms |
| Metadata-only publication, retaining the same RDF5D mapping | 10.7–11.7 ms |
| Reader refresh after graph publication | 12.6–13.0 ms |

These are one machine's diagnostic results, not performance guarantees. They
show that full-file publication takes about 0.2 seconds at the requested scale;
they do not yet justify the complexity of graph-level disk COW. Manifest-plus-mmap
open times vary, so do not treat the difference from direct mmap as a stable
backend overhead measurement.

After the initial graph publication, a one-graph replacement and a metadata-only
publication, three RDF5D objects remained: the empty initial store and two complete
populated stores, totaling about 27.81 MiB. Metadata publication added no RDF5D
object; the manifest file was 680 KiB. Retention grows by approximately one full
store per graph-changing commit, including when only one graph changes.

Process high-water RSS was approximately 166–172 MiB. This includes synthetic
source generation, parsing/staging, serialization and retained mappings; it is
not isolated reader memory or a measurement of many reader processes. Cold
reads, realistic queries, index-building memory and writer allocation attribution
still need measurement before choosing on-disk graph-level COW.

Before replacing legacy persistence, settle the manifest publication design and
its durability/availability contract. Then add recursive imports/source updates,
dependency-closure conveniences, CLI/Python adapters and explicit migration.
Online retention needs process-safe snapshot pins before any objects can be
deleted. Per-graph on-disk COW is a later option if measurements show complete
generation writes dominate; it also requires coordinating term dictionaries and
blank-node identity across graph files.
