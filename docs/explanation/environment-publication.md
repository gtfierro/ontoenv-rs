# Atomic publication belongs to OntoEnv

Status: 0.7.0-rc.1 preparation in PR #36, October 23, 2026. Immutable manifests
and an atomic `CURRENT` file are the default managed storage in Rust, Python and
the CLI. See [the snapshot guide](transactional-snapshots.md) for APIs and measurements.

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

| Component | Responsibility |
| --- | --- |
| RDF5D | Build, validate, open and query a fixed artifact |
| OntoEnv update | Stage graph replacements/deletions and candidate metadata |
| OntoEnv publication | Select one complete environment version atomically |
| OntoEnv reader | Retain one version until explicit refresh |

All publication code lives in `lib/src/snapshots.rs`; the RDF5D crate gains no
transaction machinery. The old Cargo feature name is an empty compatibility
feature; managed storage no longer requires opting in.

## Minimal contract

For a local filesystem, one writer, roughly 100 graphs and 10–20 MB of encoded
data, with bursty writes and readers explicitly refreshing:

1. Published RDF5D objects and selected manifests never change in place.
2. One selected manifest identifies catalog, configuration and graph data as a
   unit. Readers do not assemble a version from independently current files or
   filesystem modification times.
3. Graph-changing commits write a complete new RDF5D artifact. Metadata-only
   commits reuse the previous artifact. Beginning an update clones small
   metadata and holds only graph replacements/deletions in an overlay.
4. Preparation is private. Abort, Drop, parse errors and validation failures do
   not publish. Successful publication has one authority change.
5. Readers do not hold the writer's lock. Retained views keep their old mapping
   and metadata. Refresh validates a replacement before swapping it;
   a refresh error leaves the old view intact.
6. A publication I/O error may mean old or new head is durable. Further commits
   from that writer are rejected until close/reopen reconciles head.
   Its last confirmed snapshot remains readable but may be behind durable head.
7. Keep all complete objects and manifests initially. Neither retained files nor
   a generation counter promise a public history/checkout API. Reset publishes
   a replacement version. Migration is explicit and preserves legacy files.
   Online pruning is not supported.

## Disk layout and authority

```text
root/
  .ontoenv-snapshots.lock      # stable exclusive writer lock
  .ontoenv-snapshots/
    CURRENT                   # schema, manifest file name, BLAKE3 of its bytes
    manifests/
      manifest-<unique>.json   # version info, catalog, config, object reference
    objects/
      object-<unique>/
        store.r5tu            # complete immutable RDF5D artifact
```

Pointer and manifest schemas are version 2. Names are validated as restricted
single path components before use. Random names avoid collision with abandoned
attempts; version numbers describe committed versions rather than filenames.
Readers follow only `CURRENT`, validate the selected manifest's checksum/schema,
then map exactly its object. Missing/corrupt selected content is an error, not
permission to scan for another manifest. Objects retain RDF5D's existing CRC
validation; the manifest checksum does not make external mutation of mmap data
safe.

Earlier redb-based experimental environments are rejected explicitly. Neither
those environments nor legacy `.ontoenv/` stores are overwritten by normal
open/create calls. Legacy managed stores have an explicit copy migration.
The specialized Rust reader/update API remains available alongside the normal API.

## Publication protocol

1. Hold the stable writer lock. Prepare and validate the complete candidate.
2. If graphs changed, finish, validate and sync a unique RDF5D object and sync
   its containing directory entries. Metadata-only updates reuse the old object.
3. Check the expected environment identity/head under the writer lock. Write a
   unique manifest, sync its bytes, retain it, close it and sync `manifests/`.
4. Write a small temporary pointer in the directory containing `CURRENT`, naming
   that manifest and its BLAKE3 checksum. Sync its bytes and close the handle.
5. Atomically replace `CURRENT` using `std::fs::rename`, then sync its
   parent directory. Replacement is the visibility point. No unlink/truncate
   of the old pointer occurs. Prepare the writer's in-memory snapshot before
   this step so installing it afterward cannot fail through file I/O.

The expected-head check is not a compare-and-swap for processes bypassing the
stable writer lock. All cooperating writers must take that lock. Initialization
prepares version zero in a private sibling directory, then renames the complete
directory into place. The writer lock remains outside that directory and is
never replaced or removed.

A reader closes `CURRENT` and the manifest before mapping RDF5D. A concurrent
publication may mean it selects the previous version; that is a valid coherent
snapshot. Since nothing is pruned, both the selected manifest and its object
remain available without a lock spanning selection and mapping. Fresh readers
need no writer to repair files after an abrupt process exit. Unreferenced files
left by interrupted attempts are ignored and retained.

The tempfile dependency creates unique pointer files and clears temporary file
attributes before the writing handle closes. Rust's `std::fs::rename` performs
replacement without first unlinking CURRENT. On Windows it has a POSIX-semantics
fallback for replacing a file whose readers still hold it open; tempfile's
MoveFileExW-only persistence failed the concurrent-reader stress test. A regression
test now keeps the old CURRENT handle open throughout publication and checks
that it still reads the old bytes after new opens select the new pointer.
[Rust 1.88 Windows implementation](https://github.com/rust-lang/rust/blob/1.88.0/library/std/src/sys/fs/windows.rs#L1170).
Directory sync currently runs on Unix and is a no-op elsewhere. Passing Windows
process tests establishes normal-operation/process-exit behavior, not a Windows
power-loss guarantee. Filesystem and device flush semantics also matter on Unix.
A bounded guest crash/reboot campaign has now been performed on Linux/ext4;
see [validation and acceptance gates](publication-validation.md) for its results
and limits. It does not power-cycle host or device caches.

## Why replace the manifest database?

The mutable authority is one selected version. The earlier redb prototype used
experimental multiprocess support to coordinate this small head. It also needed
a writer to recover before a fresh read-only open after an unclean writer exit.
The file publisher removes that availability dependency, database handles,
internal history table and redb's higher compiler requirement. It keeps direct
RDF5D mmap access and does not introduce a public backend abstraction.

This is a simplicity/availability experiment, not a claim that files are faster
than a database. Two synced files and directory barriers have a cost. The updated
profile compares this implementation with the saved redb baseline; neither is a
controlled filesystem benchmark. Whole-file rewrites, rather than query or codec
changes, remain the chosen starting point.

## Acceptance coverage and remaining work

The Linux test suite covers explicit refresh and retained views, writer exclusion,
rollback, aliases/configuration, empty versus removed graphs, source IRI changes,
failed refresh, invalid CURRENT paths/schema/checksum, invalid selected manifests,
and explicit rejection of earlier redb environments.

Child-process tests exit at eight points: object ready, manifest written, manifest
synced, manifest directory synced, pointer written, pointer synced, pointer
replaced, and final directory synced. At every point a fresh reader opens before
any recovery writer, selecting exactly the old or new complete version. A new
writer can then publish immediately. Injected publication errors on either side
of replacement disable retry until reopen; known preparation failures allow retry.

A second test runs three reader processes opening and refreshing while a writer
publishes twenty successive changes. Each reader checks graph content, aliases
and configuration against its selected version and retains its original view.
Dedicated feature CI runs these tests on Linux, macOS and Windows at Rust 1.88;
consult the PR checks for results on the current revision. The Windows fix and
repeated stress passed on all three platforms at `ac607d9`.

Before normal Rust/Python/CLI integration, review platform durability guarantees,
measure cold reads and query-index memory across processes, and add recursive
source updates and explicit legacy migration. Online cleanup requires a separate
protocol covering sleeping readers and every retained query view. Per-graph disk
COW remains deferred until complete-file publication measurements justify changes
to dictionary ownership and cross-graph queries.
