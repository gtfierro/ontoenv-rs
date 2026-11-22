# OntoEnv publication and snapshots

Starting with 0.7.0-rc.1, immutable publication is the default storage backend
for new managed persistent environments in Rust, Python and the CLI. It is
always compiled, without a feature flag. Temporary environments and caller-provided graph stores retain their
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
use ontoenv::{SnapshotReader, SnapshotWriter};

let mut writer = SnapshotWriter::create(config)?;
let mut update = writer.begin_update();
let id = update.add_from_bytes(location, bytes, format)?;
update.add_alias("urn:alias", id.name().as_str())?;
let committed = update.commit()?;

let mut reader = SnapshotReader::open(root)?;
let old = reader.snapshot();
let old_view = old.view_graph(&id)?;
// Further writer commits do not change `old` or reader.snapshot().
reader.refresh()?;
```

These explicit entry points return `SnapshotWriter` and `SnapshotReader`.
`SnapshotWriter::open(root)` opens an existing writer. Normal
`OntoEnv::connect/create/open` also use this backend, with the existing source
and import pipeline. Their successful graph and configuration mutations publish before returning.
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
dataset = old.view_dataset()
print(old.info)  # environment ID, generation, parent, committed_at, opened_at
writer.update()
reader.refresh()  # select a complete newer version, if one exists
# old, dataset and previously returned graph/closure views keep their version.
reader.close()
writer.close()
```

`EnvironmentSnapshot` provides `get_ontology_names()`, `get_ontologies()`,
paired `view_*`/`copy_*` queries, namespace access, a copied `config` dictionary
and `info`. These handles remain usable after the environment closes.
`OntoEnv.read_snapshot(path)` opens one owned version without retaining a live
environment. A Python snapshot supports `close()`, `closed`, and `with`:

```python
with OntoEnv.read_snapshot("./ontology-env") as version:
    graph = version.view_graph("urn:example:ontology")
# The version handle is closed. The escaped graph still owns its backing.
print(len(graph))
del graph  # Releases this view's ownership; later cleanup can reclaim its file.
```

Closing releases only that handle. There is no read transaction to commit or
roll back. Rust uses ordinary ownership: dropping the last `Arc`/owned iterator
releases its backing pin.

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

`snapshot.view_graph(&id)` and `snapshot.view(&ids)` return borrowed scoped
`rdf5d::View`s. Their lifetime is tied to the selected environment snapshot, and
they validate graph identifiers against that catalog. Keep a view across queries
to reuse its scoped indexes. Those scoped indexes are independent of whole-store
indexes so scoped transitive closures cannot follow unrelated graphs.

The paired ownership diagnostic at ~19.5 MiB with four active reader processes
measured aggregate PSS of approximately 204 MiB before refresh and 204 MiB after
refresh when old handles were released. Keeping both indexed versions raised it
from approximately 204 MiB to 392 MiB. This is a diagnostic result, not a memory
guarantee; raw reports are in `docs/reviews/snapshot-ownership-2026-10-23.json`.
Releasing the last owner also releases its cross-process file pin. A subsequent
cleanup pass can reclaim the backing on disk once it is outside retained head
history.

## Commit protocol and copy-on-write

An update clones small catalog/configuration values and holds a graph overlay
of replacements and deletions. Untouched content remains in the immutable,
mmap-backed base. It does not clone the entire decoded store at transaction start.

Parsed replacements are staged as shared, deduplicated triple buffers, without
building OxRDF graph indexes. The serializer iterates those buffers directly.
Explicit staged graph reads materialize an independent graph on demand; repeated
reads can therefore repeat that work. Callers that supply an existing graph keep
that representation. Nested checkpoints share either form through `Arc`.

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
Readers briefly share the `READERS` acquisition gate while reading `CURRENT`,
validating its checksum-verified manifest, and pinning/opening the graph backing.
They then release the gate and retain a shared OS lock on that object's `.pin`
file. They hold no writer lock or database
connection. Initialization is prepared in a private directory and renamed
into place only after generation zero is complete. Recreation and reset publish
a replacement catalog; they retain old mapped files and the stable lock.
Close the active writer before recreation. Normal new-environment creation
installs an empty generation before any requested source scan; a failed initial
scan leaves an openable empty environment.

## Retention and recovery limits

Cleanup retains the **current and exact previous published manifest**, their RDF
objects, and any older object owned by a live snapshot, view or iterator in any
process. Metadata-only publications can share the same object. The previous
manifest is recorded in `CURRENT`; abandoned publication attempts are never
mistaken for published history.

The writer tries cleanup at startup and after successful publication. Use
`ontoenv prune`, `env.prune()` or Rust `SnapshotWriter::prune()` to retry sooner.
There is no background reaper: releasing the last view makes a file eligible;
the next pass removes it. A reader process exit releases its OS pins, including
after a crash. A live sleeping reader never loses its backing to a timeout.

Cleanup briefly takes the exclusive acquisition gate and nonblocking exclusive
object pins. It skips busy objects, renames eligible object directories into
`trash/`, then releases the gate before deleting them. Interrupted/failed
deletions remain there for later retry. Cleanup errors are reported by explicit
prune and logged by automatic passes; they do not turn a successful publication
into a failed commit. Current/previous manifests and active pins are never
deleted. Do not manually delete or move files in a running store.

Reader protocol 3 introduces this ownership contract. Protocol-2 stores from
earlier unreleased builds require the explicit
[offline upgrade](../how-to/migrate-storage.md); all older readers must be closed
first because their files are unpinned. Legacy `.ontoenv/` migration remains a
separate operation.

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

## Windows 0.7 release contract

The accepted Windows contract covers coherent publication and owned read views
under normal operation and abrupt process exit. After a writer process exits,
readers select a complete published version; retained views keep their captured
version. A publication I/O error still requires closing and reopening the writer
to reconcile its selection before retrying.

Power-loss durability is **best effort**. A host crash or power failure can lose
a successful update or leave the environment unable to open. Neither a returned
commit nor retention of the previous version guarantees that even one complete
version will be recoverable after power loss. Rebuilding the environment from
ontology sources may be necessary; invalid selected state fails explicitly
rather than exposing a mixture of versions.

Existing file flushes and integrity checks remain enabled. Directory syncing is
unsupported on Windows, and 0.7 does not add automatic history fallback or claim
Unix-equivalent power-loss durability. This limit is accepted for the local,
infrequently written dependency-graph workload; it does not weaken the owned-view
or cooperating-process consistency contract.

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

A later normal-API diagnostic used 100 graphs, 334,059 named-graph quads and a
13.8 MiB encoded store. Compact parsed staging reduced native initial-import peak
RSS from 534–544 MiB to 223–225 MiB, and Python from 570–572 MiB to 257–260 MiB.
The complete Python workload, including retained views and repeated updates,
still peaked at approximately 431–450 MiB. These are different measurement scopes,
not supported memory ceilings. Three alternating baseline/optimized runs also
observed slower first closure construction (median 70 to 88 ms), which is tracked
separately. Commands, allocation profiles and limitations are recorded in
`docs/reviews/compact-staging-2026-10-23.md` and its raw reports.

The release candidate integrates recursive source updates, dependency views,
CLI/Python entry points, explicit legacy migration and reader-safe cleanup.
Long-lived views retain entire old RDF files; graph changes still rewrite the
complete store. Per-graph on-disk COW is a later option if measurements show complete
generation writes dominate; it also requires coordinating term dictionaries and
blank-node identity across graph files.
