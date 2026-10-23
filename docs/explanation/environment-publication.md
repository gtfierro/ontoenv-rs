# Atomic publication belongs to OntoEnv

Status: design review of the opt-in prototype in PR #36, October 23, 2026.
This describes the intended boundary and remaining acceptance work; it does not
change the default persistence API. See [the prototype](transactional-snapshots.md)
for the implemented Rust API and measurements.

## The boundary

RDF5D is an immutable, queryable storage artifact. It owns encoding, validation,
file construction, mmap access, triple lookup and query indexes. Its `Snapshot`
is an owned query handle to fixed file contents. Building query indexes lazily
inside that handle does not change the published file.

OntoEnv owns source discovery, ontology identity, aliases, imports, configuration
and publication of complete environment versions. Writer exclusion, version
numbers, commit timestamps, refresh and recovery belong here. A version bundles
the catalog and configuration with a reference to an immutable RDF5D file.
There is no independent catalog save after graph publication.

| Component | What it does | What this design does not add |
| --- | --- | --- |
| RDF5D | Build, validate, open and query a fixed artifact | Environment transactions, writer locks or version discovery |
| OntoEnv update | Stage graph replacements/deletions and candidate metadata | Concurrent mutations of a mapped file |
| OntoEnv publication | Select one complete environment version atomically | Record-level transactions or multiple simultaneous writers |
| OntoEnv reader | Retain one version until explicit refresh | Implicit source updates during reads |

PR #36 already follows this boundary: its publication code is in
`lib/src/snapshots.rs`; the prototype adds no transaction machinery to the RDF5D
crate. Keep the experimental API/feature names for now rather than mixing a
terminology rename into persistence changes.

## Minimal contract

For a local filesystem, one writer, roughly 100 graphs and 10–20 MB of encoded
data, with bursty writes and readers explicitly refreshing:

1. A published RDF5D object is never changed in place.
2. The selected version identifies catalog, configuration and graph data as one
   unit. Readers select it once; they do not assemble it from separate current
   files or filesystem modification times.
3. Graph-changing commits write a complete new RDF5D artifact. Metadata-only
   commits reuse the previous artifact. Beginning an update clones small
   metadata and holds only graph replacements/deletions in an overlay.
4. Preparation is private. Abort, Drop, parse errors and validation failures do
   not publish. Successful publication has one authority change.
5. Readers do not hold the writer's lock. Retained views keep their old mapping
   and metadata. Refresh builds and validates a replacement before swapping it;
   a refresh error leaves the old view intact.
6. A publication I/O error may mean old or new head is durable. Further commits
   from that writer are rejected until close/reopen reconciles durable head.
   Its last confirmed snapshot remains readable but may be behind durable head.
7. Keep all complete objects initially. Neither retained files nor a generation
   counter promise a public history/checkout API. No reset, migration or online
   pruning is part of this first contract.

## Manifest choices

The mutable authority is small: select one complete version. Neither choice
below changes RDF5D or replaces its mmap access with database blob reads.

| Concern | Current redb manifest | Immutable manifests and an atomic `CURRENT` file |
| --- | --- | --- |
| Selected state | One short transaction reads the complete head record | Read `CURRENT` once, then the named immutable manifest |
| Publication | Durable database commit after object durability | Sync object and manifest, replace `CURRENT`, then sync its parent |
| Writer exclusion | Stable OntoEnv writer lock plus database locking | Stable OntoEnv writer lock |
| Live readers | Experimental redb multiprocess mode | Readers briefly open pointer/manifest files; retain RDF5D mappings |
| Unclean writer exit | New read-only opens require a repairing writer with the pinned backend | Intended to need no writer recovery; must be demonstrated with crash tests |
| Dependencies | redb 4.3.0, experimental multiprocess feature, Rust 1.90+ | Filesystem publication implementation and platform-specific durability work |
| History | Internal generation table, no public history API | Retained manifests, no public history API required |

The pinned redb implementation was inspected locally, including its read-only
recovery tests in `src/db.rs`. OntoEnv's child-process test now also checks that
fresh read-only open is refused after an abrupt writer exit, before opening a
writer for recovery. Retained RDF5D views continue reading during this interval.
This is an availability limitation of the current prototype, rather than a
reason to move transaction logic into RDF5D.

For this single-record workload, **prefer an immutable-manifest publication
design as the next focused experiment**. It potentially removes the manifest
database and its recovery dependency. Keep redb as the current draft's reference
implementation until the replacement passes the same failure and concurrency
tests. Do not add a public backend abstraction or maintain two supported
backends merely to conduct that experiment.

The alternative protocol must be concrete before replacing redb:

1. Hold the stable writer lock and prepare/validate the full candidate.
2. Write the unique RDF5D object if needed; sync it and its directory entries.
3. Write a unique immutable manifest containing environment identity, version,
   parent, time, object reference/length, catalog and configuration. Sync its
   bytes and the directory that makes it discoverable.
4. Write a small temporary `CURRENT` file in the same directory as `CURRENT`.
   Include the manifest's unique name and a digest of its bytes. Sync the file.
5. Atomically replace `CURRENT` and sync its parent directory before returning
   durable success. Any uncertain publication error disables further commits
   until reopen. An atomic rename is the visibility point; it alone does not
   establish the complete crash-durability contract.
6. A reader reads one complete `CURRENT`, validates the named manifest, then
   opens exactly the referenced object. It may legitimately see the previous
   version if publication races with open. Refresh follows the same sequence.

Use restricted single-component names for manifests/objects and version their
schema. Unique names prevent abandoned attempts from colliding with later ones.
Because nothing is pruned, a reader needs no lock spanning selection and mmap.
Do not scan for the highest numbered manifest or use mtimes as a recovery rule.
Missing/corrupt selected content is an error, not permission to select another
version silently.

Readers must close pointer and manifest handles promptly. Verify replacement
with concurrent opens on Windows, and verify platform-specific sync behavior.
Rust documents different `rename` implementations on Unix and Windows; it does
not by itself supply this complete publication protocol.
[Rust filesystem documentation](https://doc.rust-lang.org/std/fs/fn.rename.html).
The current prototype's `sync_directory` performs a sync on Unix and is a no-op
elsewhere, so passing Windows process tests does not establish power-loss
durability there.

## Review findings and acceptance work

The current reader selects metadata in one read transaction, opens an immutable
object, and installs it only after validation. No pruning means ending the
manifest transaction before opening the object is safe. Graph files are prepared
before manifest publication, and the writer prepares its local snapshot before
the durable publication point. Initialization uses a private directory and a
stable external writer lock.

One gap was found and fixed: a database publication error previously left the
writer usable with a possibly stale in-memory head. The writer now refuses later
commits until reopen. Fault-injection tests cover both durable outcomes and
verify a rejected retry creates no further object. Known failures before the
publication call still allow retry.

Existing acceptance coverage includes empty versus removed graphs, explicit
aliases and configuration, source IRI changes, strict-import rollback, failed
refresh preserving old state, child-process readers alongside a live writer,
writer exclusion, and abrupt process exits before and after publication. These
are process-crash tests, not a simulated power-failure study.

Before adopting an alternative manifest publisher, add overlapping multi-reader
open/refresh loops while a writer repeatedly publishes, and crash injection
around each manifest/pointer step. Establish post-crash read-only availability
without a recovery writer. Run the matrix on Linux, macOS and Windows. Record
supported durability guarantees separately from successful normal operation.

Whole-file rewrite measurements should decide whether more disk COW is needed.
Do not split graph files, change dictionary ownership, add a WAL, or implement
online GC in this review. Cold reads, query-index memory across processes, and
writer peak memory still require workload-specific measurement. The diagnostic
profile separates source staging, full-file publication, metadata-only commits,
refresh and retained bytes; warm opens do not measure cold-cache behavior.

Only after the publication choice and failure contract are settled should we
adapt the normal Rust/Python/CLI paths, add recursive source updates and explicit
legacy migration. Automatic cleanup remains separate and must account for every
retained view and sleeping reader.
