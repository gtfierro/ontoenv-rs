# OntoEnv publication and snapshots

Starting with 0.7.0-rc.1, immutable publication is the default storage backend
for new managed persistent environments in Rust, Python and the CLI. It is
always compiled; `transactional-snapshots` remains an empty compatibility
feature. Temporary environments and caller-provided graph stores retain their
existing behavior and their backend's transaction guarantees.

Publication coordinates an OntoEnv catalog, configuration and immutable RDF5D
artifact. RDF5D remains the file builder and query engine. OntoEnv implements
writer exclusion, commit and refresh with immutable JSON manifests and an atomic
`CURRENT` file, without a database dependency. See [the design review](environment-publication.md)
and [validation report](publication-validation.md) for the protocol and measured limits.

Existing `.ontoenv/` environments require [explicit migration](../how-to/migrate-storage.md).
Migration copies the stored RDF and metadata, retains the original files, and
publishes the new directory only after validation. Earlier redb-based prototype
environments are rejected without automatic conversion.

## Try it

```sh
cargo run -p ontoenv --example transactional_snapshots
cargo test -p ontoenv --lib --test test_snapshots --test test_concurrency
```

The example keeps a writer open, opens a reader, publishes new graph content and
an alias, then refreshes the reader. A retained old query view still reads the
old generation.

```rust
use ontoenv::api::OntoEnv;

let mut writer = OntoEnv::create_snapshot(config)?;
let mut update = writer.begin_update();
let id = update.add_from_bytes(location, bytes, format)?;
update.add_alias("urn:alias", id.name().as_str())?;
let committed = update.commit()?;

let mut reader = OntoEnv::open_snapshot(root)?;
let old = reader.snapshot();
let old_view = old.graph_view(&id)?;
// Further writer commits do not change `old` or reader.snapshot().
reader.refresh()?;
```

These explicit entry points return `SnapshotWriter` and `SnapshotReader`.
`OntoEnv::open_snapshot_writer(root)` opens an existing writer. Normal
`OntoEnv::connect/create/open` also use this backend, with the existing source
and import pipeline. Their successful graph mutations publish before returning.
`env.snapshot()` captures the selected committed version; `env.refresh()`
advances a managed read handle without reading sources. Rust callers can group
operations with `env.transaction(|env| { ...; Ok(()) })`; nested errors and
unwinding restore staged graphs, catalog, configuration and runtime settings.

Python uses the same lifecycle:

```python
from ontoenv import OntoEnv

writer = OntoEnv.connect("./env", offline=True)
writer.add("./ontology.ttl")
reader = OntoEnv.open("./env", read_only=True)
old = reader.snapshot()
dataset = old.get_dataset()
print(old.info)  # environment ID, generation, parent, committed_at, opened_at
writer.update()
reader.refresh()  # select a complete newer version, if one exists
# old, dataset and previously returned graph/closure views keep their version.
reader.close()
writer.close()
```

`EnvironmentSnapshot` provides `get_ontology_names()`, `get_ontologies()`,
`get_graph()`, `get_dataset()`, namespace access, a copied `config` dictionary
and `info`. These handles remain usable after the environment closes.
`get_graph`, `get_dataset`, `get_closure` and `get_union` views pin their backing
version. Fetch another view to see an update, or explicitly call
`env.refresh_dataset(dataset)` to rebind that dataset to the handle's selected
version. Existing iterators finish on their original backing and term cache.
Reader refresh and dataset rebind are separate operations; `flush` does not
advance readers or mutate already returned managed views.

Each managed version contains one graph per canonical IRI and one registration
per source. A replacement updates that registration and preserves its explicit
aliases. Multiple source versions of one IRI must be assigned distinct graph
IRIs; duplicate legacy registrations are rejected during migration.

Readers expose catalog listings, aliases, configuration, graph copies, generation
information and the existing RDF5D `Snapshot`/`View` engine. They have no mutation
or save methods. `committed_at` records the time while preparing publication;
generation numbers, not timestamps, order commits. `opened_at` records this
process's snapshot creation time.

## Snapshot and index ownership

`reader.snapshot()` clones an `Arc` to the reader's selected environment version;
it does not reopen the file. Calls to `snapshot.rdf()` clone the same RDF5D `Arc`,
so they share that object's lazy whole-store indexes. Independently opened
readers and separate processes own their own heap indexes; there is no global
snapshot registry.

An ordinary graph-changing refresh replaces the reader's owning `Arc`. If no
caller retains the old snapshot, view or iterator, its mapping and indexes become
reclaimable. A retained environment snapshot keeps its metadata and graph backing;
an owned RDF5D grouped iterator keeps only the RDF backing/indexes it needs. These
lifetimes are covered with weak-reference tests. Allocator behavior still controls
when freed heap pages leave RSS.

Metadata-only refresh installs new catalog/configuration values while reusing
the same immutable RDF5D mapping and whole-store indexes. Old retained metadata
does not change. Refresh with an unchanged generation leaves the existing
environment snapshot itself untouched.

`snapshot.graph_view(&id)` and `snapshot.view(&ids)` return borrowed scoped
`rdf5d::View`s. Their lifetime is tied to the selected environment snapshot, and
they validate graph identifiers against that catalog. Keep a view across queries
to reuse its scoped indexes. Those scoped indexes are independent of whole-store
indexes so scoped transitive closures cannot follow unrelated graphs.

The paired ownership diagnostic at ~19.5 MiB with four active reader processes
measured aggregate PSS of approximately 204 MiB before refresh and 204 MiB after
refresh when old handles were released. Keeping both indexed versions raised it
from approximately 204 MiB to 392 MiB. This is a diagnostic result, not a memory
guarantee; raw reports are in `docs/reviews/snapshot-ownership-2026-10-23.json`.
Memory reclamation does not prune retained files on disk.

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

Dropping or aborting an explicit `SnapshotUpdate` publishes nothing. Its strict
commits reject unresolved imports across the complete candidate; `add_source`
and `add_from_bytes` stage one ontology each, so stage their dependencies too.
The normal `OntoEnv` API retains recursive imports, configured filters, strict
and non-strict behavior and explicit import depth limits. That pipeline validates
its candidate before publication. “Add with rename” publishes once, including
both ingestion and renaming.

The sole writer holds a stable `.ontoenv-snapshots.lock` outside the data directory.
Readers briefly read `CURRENT` and its checksum-verified immutable manifest,
then retain the immutable graph mapping. They hold no writer lock or database
connection. Initialization is prepared in a private directory and renamed
into place only after generation zero is complete. Recreation and reset publish
a replacement catalog; they retain old mapped files and the stable lock.
Close the active writer before recreation. Normal new-environment creation
installs an empty generation before any requested source scan; a failed initial
scan leaves an openable empty environment.

## Retention and recovery limits

Published manifests and complete objects, including unreferenced files, are
**retained indefinitely**. Reset does not reclaim disk space. Automatic pruning
is not exposed. Do not
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
cargo run --release -p ontoenv --example snapshot_profile -- 100 500
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

The release candidate integrates recursive source updates, dependency views,
CLI/Python entry points and explicit legacy migration. The remaining operational
limit is indefinite retention. Online retention needs process-safe snapshot pins before any objects can be
deleted. Per-graph on-disk COW is a later option if measurements show complete
generation writes dominate; it also requires coordinating term dictionaries and
blank-node identity across graph files.
