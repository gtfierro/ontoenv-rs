# Experimental OntoEnv publication and snapshots

Publication coordinates an OntoEnv catalog, configuration and immutable RDF5D
artifact. RDF5D itself remains a file builder and query engine; it does not
implement environment transactions. The feature name remains
`transactional-snapshots` while this is experimental. See
[the boundary and manifest review](environment-publication.md) before treating
this prototype as a production storage design.
The subsequent [validation report](publication-validation.md) records the Windows
replacement fix, guest crash/reboot campaign, larger multi-reader workloads and
provisional acceptance budgets.

The `transactional-snapshots` feature adds an experimental Rust storage API:
`SnapshotWriter`, `SnapshotUpdate`, `SnapshotReader`, and `EnvironmentSnapshot`.
It uses immutable JSON manifests and an atomic `CURRENT` file. The redb
dependency has been removed; feature CI now tests the workspace Rust 1.88 floor.
Earlier redb-based prototype environments are rejected without automatic migration.

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

The candidate file is synced and validated before writing and syncing an immutable
manifest with its reference, metadata, aliases, configuration and next generation.
A synced temporary pointer atomically replaces `CURRENT`; OntoEnv then syncs its
parent directory and installs the already-prepared committed snapshot. A metadata-only update
reuses the same mapped file and performs no RDF5D rewrite.

Dropping or aborting an update publishes nothing. Strict commits reject unresolved
imports across the complete candidate. `add_source` and `add_from_bytes` currently
stage one ontology; stage its dependencies in the same transaction. Recursive
discovery/fetching and non-strict failure reports are not implemented yet.

The sole writer holds a stable `.ontoenv-snapshots.lock` outside the data directory.
Readers briefly read `CURRENT` and its checksum-verified immutable manifest,
then retain the immutable graph mapping. They hold no writer lock or database
connection. Initialization is prepared in a private directory and renamed
into place only after generation zero is complete. Recreation is deliberately
unsupported rather than deleting a live lock or mapped files.

## Retention and recovery limits

Published manifests and complete objects, including unreferenced files, are
**retained indefinitely**. Automatic pruning, reset and migration are not exposed. Do not
delete old objects manually while any reader or query view can use them. This
initial policy avoids GC races and lets sleeping readers retain snapshots.

A process exit before `CURRENT` replacement leaves the previous generation selected;
it may leave unreferenced files. An exit after replacement selects the complete
new generation. A publication I/O error may have an uncertain durable outcome:
close/reopen the writer and inspect the committed generation before retrying.
Further commits on that writer are rejected; its last confirmed snapshot may
lag durable head. A known failure before attempting publication permits retry.
Fresh readers open after an abrupt writer exit without a recovery writer. They
select only `CURRENT`, never an unreferenced manifest with a higher version.
Malformed pointers, missing manifests and checksum failures produce errors;
failed refresh preserves the old snapshot.

Publication interruption and repeated multi-reader open/refresh tests pass
locally on Linux, with dedicated Linux/macOS/Windows CI. These process-exit tests
do not establish power-loss durability. Directory syncing is implemented on Unix
and is currently a no-op elsewhere. External mutation/truncation of an
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

Three sequential release-mode runs per backend on October 23, 2026, with 100
graphs and 50,100 triples produced approximately 13.91 MiB RDF5D stores. They
used rustc 1.99.1 on x86_64 Linux. The ranges span three runs, not confidence
intervals. Raw output, commands and source hashes are retained in
`docs/reviews/snapshot-profile-2026-10-23.txt` (earlier redb baseline) and
`docs/reviews/snapshot-profile-files-2026-10-23.txt` (current file publisher).
These are sequential diagnostics, not randomized controlled comparisons.

| Operation | Earlier redb | Current files |
| --- | --- | --- |
| Initial source generation, parsing and staging | 129–133 ms | 121–130 ms |
| Initial serialization/publication (after source parsing) | 166–182 ms | 183–190 ms |
| Warm direct RDF5D mmap open | 11.8–12.7 ms | 12.2–12.9 ms |
| Warm direct RDF5D owned-byte open | 12.7–13.4 ms | 12.8–13.2 ms |
| Warm manifest plus RDF5D mmap open | 12.1–21.5 ms | 13.7–17.7 ms |
| One-graph source generation, parsing and staging | 1.0–2.2 ms | 2.2 ms |
| One graph changed, complete-file serialization/publication | 179–211 ms | 222–227 ms |
| Metadata-only publication, retaining the same RDF5D mapping | 10.7–11.7 ms | 22.8–23.9 ms |
| Reader refresh after graph publication | 12.6–13.0 ms | 17.9–18.5 ms |

These are one machine's diagnostic results, not performance guarantees. They
show that full-file publication takes about 0.2 seconds at the requested scale.
The file publisher pays for two file syncs plus directory barriers; metadata-only
publication was about twice as slow as the redb baseline. Keep those barriers
until a stronger durability analysis justifies changing them. This experiment
simplifies coordination and removes reader recovery; it is not a demonstrated
write-speed improvement. Manifest-plus-mmap open times vary, so do not treat the
difference from direct mmap as a stable backend overhead measurement.

After the initial graph publication, a one-graph replacement and a metadata-only
publication, three RDF5D objects remained: the empty initial store and two complete
populated stores, totaling about 27.81 MiB. Metadata publication added no RDF5D
object. Four retained JSON manifests totaled approximately 108.8 KiB, plus a
137-byte CURRENT file, versus a 680 KiB manifest database in the baseline.
These are logical file lengths, excluding filesystem allocation overhead.
Retention grows by approximately one full store per graph-changing commit,
including when only one graph changes, plus one manifest per commit.

Process high-water RSS was approximately 164–168 MiB (baseline: 166–172 MiB).
This includes synthetic
source generation, parsing/staging, serialization and retained mappings; it is
not isolated reader memory or a measurement of many reader processes. Cold
reads, realistic queries, index-building memory and writer allocation attribution
still need measurement before choosing on-disk graph-level COW.

Before replacing legacy persistence, review the file publication protocol and
its platform durability contract. Then add recursive imports/source updates,
dependency-closure conveniences, CLI/Python adapters and explicit migration.
Online retention needs process-safe snapshot pins before any objects can be
deleted. Per-graph on-disk COW is a later option if measurements show complete
generation writes dominate; it also requires coordinating term dictionaries and
blank-node identity across graph files.
