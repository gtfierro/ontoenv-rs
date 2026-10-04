# Transactional state review and snapshot proposal

Review date: 2026-10-04. Scope: the Rust environment/catalog/RDF5D persistence paths, with a check of the custom-backend interface and Python batch hooks. This is a focused consistency review, not an exhaustive audit of RDF5D parsing or all Python behavior. No production implementation was changed.

Workload supplied by the user: one machine, local filesystem, approximately 100 graphs and 10–20 MB stored, infrequent bursty writes, one active writer, and multiple readers that explicitly refresh their snapshots.

## Findings

### 1. High: failed operations persist partial graph mutations

`lib/src/api.rs: BatchScope::run` calls `end_batch()` and `flush()` before examining whether the operation succeeded. `PersistentGraphIO::end_batch` writes dirty state when the outer batch ends. `BatchScope::drop` also ends the batch during unwinding. The `EnvTransaction` rollback explicitly excludes backend mutations. `PersistentGraphIO::drop` may write dirty state again.

Reproduced: add a valid ontology importing a missing local file in strict mode. `add` returns an error, but the valid seed graph is already persisted. A pending marker remains and a subsequent normal open requires recovery. The marker detects some incomplete operations; it does not supply rollback or preserve the prior readable committed state.

Required behavior: stage graph bytes and metadata privately. Commit both only on success; error and Drop discard staging. In non-strict mode, committing successful imports alongside a durable failure report can remain intentional, but it must still be one coherent commit.

### 2. High: empty replacement resurrects old triples

`lib/src/io.rs: PersistentGraphIO::write_store_to_r5tu` decides whether a graph was written using `written_graphs`, populated by iterating quads. A resident graph replaced with an empty graph contributes no quads. The fallback copying loop then copies its old triples from `r5_file` because it is still in `r5_index`.

Reproduced: write a one-triple graph, reopen the backend, replace it with an empty graph. The live handle reads zero triples, but a newly opened backend reads the original triple.

Required behavior: distinguish untouched graphs, replacements (including empty), and deletions explicitly. Persist graph existence separately from triples; an empty graph and a deleted graph need distinct semantics. Full-file serialization must never interpret “zero emitted quads” as “copy the old graph.”

### 3. High: read-only environment handles can persist metadata

`OntoEnv` has no explicit access-mode field. `save_to_directory`, `add_alias`, `remove_alias`, and `refresh_from_store` do not reject read-only handles. The graph backend blocks many graph mutations, but catalog/config writes bypass it.

Reproduced: open with `read_only=true`, call `add_alias`, and inspect the catalog directly: the new alias is persisted. Two shared-lock readers can therefore concurrently replace catalog/config state and lose each other's updates. Some graph mutation entry points can also create a pending marker before the backend rejects the operation.

Required behavior: enforce access mode at the environment API boundary. Prefer separate `Snapshot` and `Writer` types, so snapshots have no persistence methods. For custom backends, carry the caller's access mode explicitly rather than inferring it from the backend's `io_type()` string.

### 4. High: recreation/reset invalidate the lock protocol

`OntoEnv::init(config, overwrite=true)` removes `.ontoenv` before acquiring the writer lock. `reset` also removes it without locking. On Unix, a live process retains its lock on the unlinked old lock inode; initialization creates and locks a different inode at the same pathname.

Reproduced on this Linux workspace: keep the first persistent writer open and initialize the same root with overwrite enabled. Both writer handles exist successfully. The old handle may subsequently publish into the new environment's paths.

Required behavior: maintain a stable lock inode outside anything that reset/recreation deletes. Destructive lifecycle operations must participate in the same writer serialization and snapshot-retention protocol.

### 5. High: store, catalog, and configuration have separate publication points

RDF5D's writer does a good individual-file atomic replacement: temporary file, file sync, rename, and on Unix parent-directory sync. However, environment publication is graph store first, then configuration, then catalog, then pending-marker deletion. There is no atomic commit spanning the files.

`write_json_file` truncates configuration in place and does not sync it. Pending-marker creation/deletion does not sync the directory. Metadata-only operations can write without a pending marker. A process crash or power interruption can leave new graphs with old metadata, malformed configuration, or uncertain marker durability. A post-rename directory-sync error can also report failure after a new RDF5D file has become visible.

Required behavior: one durable commit point for graph references, ontology metadata, aliases, resolution policy, persistent configuration, and revision identity. Recovering uncommitted staging should not require discarding source/provenance metadata and rebuilding everything from graph content.

### 6. Functional mismatch: readers conflict with writers

`acquire_environment_lock` takes a lifetime shared lock for a reader and a lifetime exclusive lock for a writer. Read-only open can block indefinitely while a long-lived writer exists. A writer cannot acquire its lock while readers exist. The pending-marker precheck sometimes returns an error after a two-second wait instead.

The existing concurrency tests exercise reader/reader and writer/writer combinations, not a reader opening and continuing to read while a writer commits. Both existing tests passed during this audit.

Required behavior: reader handles select committed generations without holding the writer lock. Writers serialize publication, preferably with short database write transactions.

### 7. Medium: persisted explicit aliases disappear on reopen

`load_from_directory` and custom-backend connect normalize file locations. `Environment::normalize_file_locations` calls `rebuild_aliases`, which clears all aliases and recreates only URL-location aliases.

Reproduced: add an explicit alias, close, reopen; `resolve_alias` returns None. The alias was present in the saved catalog before open normalization.

Required behavior: persist user aliases independently from derived location aliases, and preserve user aliases across normalization and refresh. Full backend refresh currently constructs a new environment from graph content and therefore also loses metadata that cannot be recovered from triples.

### 8. Medium: refresh and revision checks do not constitute a snapshot

`refresh_from_store` scans graph IDs, reads graph content, and obtains revision information through independent backend calls. It lacks a backend snapshot handle. Targeted refresh mutates live metadata incrementally; a later graph-read error can leave partial in-memory refresh state. Full refresh builds metadata privately, which is better, but does not guarantee the backend scan represents one revision.

Custom-backend connect loads catalog state before `Self::new` acquires the environment lock. Built-in load checks its pending marker before acquiring its lock and reads configuration before locking. A waiting open can therefore miss an interrupted mutation created between the precheck and lock acquisition. Recovery also constructs its built-in backend before acquiring the environment lock.

The persistent backend holds the RDF5D mapping/index opened at construction, but `store_state()` stats the current pathname. External atomic replacement can make content reads refer to the old file while revision reporting refers to the new file. Built-in backends do not implement per-graph revisions. `length:mtime` is not a reliable unique commit identity.

Required behavior: bind metadata, graph IDs, content, and revision reporting to one snapshot object. Refresh should construct an entirely new snapshot and swap it into the handle only after successful validation. Use a persistent environment UUID and monotonic generation, not filesystem timestamps, for identity and ordering.

## Undefined behavior boundary

No Rust memory-undefined-behavior defect was demonstrated in these probes. Most findings above are logical consistency and durability bugs.

`R5tuFile::open_mmap_with_options` uses an unsafe file-backed mmap. The built-in atomic replacement strategy avoids modifying the mapped inode, which is appropriate. Arbitrary external in-place writes/truncation are outside that guarantee and can violate mmap safety assumptions or cause faults. Revision checks after reading cannot make those operations safe. Immutable files, owned bytes, or a transactional database should establish the content lifetime contract explicitly.

## Recommended architecture for this workload

Use SQLite through `rusqlite`, with RDF5D retained as the graph serialization format. SQLite is responsible for transactions, revision publication, and persistence; RDF5D is responsible for encoding graph bytes and query decoding. SQL does not need to store individual triples.

SQLite WAL supports concurrent readers and a writer with snapshot isolation across connections/processes on a local machine. Long-lived read transactions can prevent checkpoint progress. Therefore do not hold a WAL read transaction open for the lifetime of a reader process. Sources: [SQLite isolation](https://www.sqlite.org/isolation.html), [SQLite WAL](https://www.sqlite.org/wal.html), [rusqlite](https://docs.rs/rusqlite/latest/rusqlite/).

Suggested initial schema:

```text
environment(environment_uuid, head_generation)
commits(generation, parent_generation, recorded_at, schema_version,
        catalog_payload, config_payload)
graph_blobs(blob_id, codec_version, checksum, rdf5d_bytes)
commit_graphs(generation, graph_id, blob_id)
```

The catalog payload can initially preserve the existing serde representation instead of introducing SQL tables for every ontology property. Include the complete source records, aliases, policy, namespaces, and any persisted failure report. A graph manifest row exists even when its graph contains zero triples. Blob IDs can be hashes of the stored bytes; do not assume RDF5D encoding is canonical across semantically equivalent RDF graphs or blank-node renamings.

Writer flow:

1. Fetch and parse sources outside the database write transaction. Construct graph replacements/deletions and candidate metadata privately. Hash the exact source bytes parsed, not a second filesystem read.
2. Begin a short write transaction (`BEGIN IMMEDIATE`). Read and verify the expected head generation. Reject or explicitly rebase a stale candidate; the check remains useful even with a single writer process.
3. Insert new immutable blobs, the complete generation manifest/catalog/config, and update head in that same transaction. Use an appropriate durable configuration, initially WAL plus `synchronous=FULL`.
4. Commit. Only then swap the writer's visible committed snapshot. A failed transaction or Drop publishes nothing.

Reader flow:

1. Open a read-only database connection. Begin a short read transaction and read head, its commit payloads, and all referenced serialized graph bytes under that transaction.
2. Retain those immutable bytes in the process, commit/end the read transaction, and build lazy graph decoders from the bytes. At 10–20 MB per reader this is a reasonable starting tradeoff, subject to measuring actual decoded-memory use and reader count.
3. All ontology listings, closures, aliases, graph queries, and metadata come from that one retained generation. Writer commits do not change it.
4. `refresh()` loads and validates a replacement snapshot, then swaps it in as a unit. Existing query/view objects retain their old `Arc<Snapshot>`.

This initial approach avoids database reader-pin writes and WAL retention for long-lived readers. Garbage collection can retain recent generations or user-pinned historical commits; already-open snapshots remain valid because they own their bytes. If eager loading becomes too expensive, add lazy historical-generation reads and an explicit cross-process retention mechanism before pruning old blobs. Do not rely solely on a PID or an expiring lease without defining behavior for sleeping readers.

Expose both `committed_at` (a timestamp recorded with the commit, not a precision guarantee about fsync completion) and `opened_at`. Include environment UUID, generation, and parent generation. Use generation for ordering; wall clocks can move backward. `is_current()` may compare against head without altering the snapshot.

Separate these operations:

- `snapshot.refresh()`: load the newest committed environment, no source discovery or writes.
- `writer.update_sources()`: discover/fetch/parse files and URLs, publish a new generation.
- `writer.reconcile_backend()`: explicitly import external backend changes under that backend's snapshot contract.

Filesystem sources themselves are not a transactional dataset: unrelated source files may change during an update. The promise is a coherent commit of the bytes actually ingested. If a source-level point-in-time view is needed, require a source snapshot such as a Git commit or immutable supplied inputs.

## RDF5D/API implementation work

- Add byte-oriented encoding/decoding (`encode -> Vec<u8>`, `open_bytes(Arc<[u8]>)` or equivalent validated owned backing). Current public reader constructors use file paths. Reuse existing bounds/CRC validation for byte-backed readers; do not bypass it.
- Keep one RDF5D blob per graph initially, with its dictionary contained in the blob. Explicitly preserve empty graphs through the manifest. Optimize cross-graph dictionaries only if measurements justify the additional version coordination.
- Make any decoded Oxigraph store or query index a disposable, revision-scoped cache. Prevent mutable store clones from bypassing commits. Cache keys need generation/blob identity, not graph IRI alone.
- Replace `begin_batch/end_batch` durability semantics with a transaction object exposing explicit commit and abort. Nested scopes must stage into the same transaction or use savepoints; Drop aborts.
- Replace string-based access mode with read-only and writer capabilities/types. Persist catalog/config changes through the same writer transaction as graphs.
- Extend custom `GraphIO` capabilities with a true read snapshot and write transaction protocol. Optional revision tokens alone are insufficient. For adapters that cannot supply snapshots/transactions, explicitly offer weaker semantics or copy data into the managed store; do not claim atomicity across an independently committed external backend and local catalog.
- Migrate the old store/catalog/config under exclusive ownership into one initial SQLite generation. Validate referential consistency, preserve provenance/explicit aliases, and leave the original files available until migration succeeds. Interrupted old pending markers require an explicit recovery policy.

## Other storage choices

| Choice | Fit |
| --- | --- |
| SQLite + RDF5D BLOBs | Recommended starting point: puts graph bytes and metadata inside the same transaction; modest memory copying is acceptable to measure for this workload. |
| SQLite manifest + immutable RDF5D files | Preserves direct mmap and lazy reads. Publish/fsync blobs before committing references. Requires explicit GC/pinning and backup coordination; database transactions do not roll back external file writes. Start here only if mmap materially matters. |
| Immutable full RDF5D generations + atomic HEAD | Feasible for 10–20 MB bursty writes and closest to Git. Include catalog/config in each generation and sync everything before replacing HEAD. Requires implementing stable writer locks, reader retention, GC, crash handling, and backups ourselves. |
| LMDB through heed | Viable transaction/snapshot alternative. Requires care around map sizing, reader lifetimes, and bytes borrowed from transaction pages. [heed documentation](https://docs.rs/heed/latest/heed/struct.Env.html). |
| redb | Attractive pure-Rust option, but standard documented open modes exclude a writer from other processes. Current source includes experimental multiprocess support; evaluate that explicitly rather than equating in-process MVCC with this workload. [Builder documentation](https://docs.rs/redb/latest/redb/struct.Builder.html), [source](https://docs.rs/redb/latest/src/redb/db.rs.html). |

At the supplied scale I would prototype SQLite BLOB storage first, measure snapshot open/query memory and burst commit latency, and only introduce external immutable files if copying or decoded memory is actually a problem.

## Validation and next steps

Four diagnostic probes were run successfully: empty replacement resurrection; alias loss/read-only metadata persistence; failed-import partial persistence; recreation bypassing a live writer lock. They deliberately assert the observed defects, not desired semantics. Their source is retained in `transactional-state-probes.rs` alongside this report, outside the CI test directory.

Commands run:

```text
cargo test -p ontoenv --test state_review_probe -- --nocapture
  4 diagnostic probes passed (confirmed observed bugs)
cargo test -p ontoenv --test test_concurrency
  2 passed, 2 helper tests ignored
```

The redesign's acceptance tests should cover reader generation N remaining coherent while a writer publishes N+1; refresh swapping all state at once; empty/deleted graphs; read-only rejection before any filesystem mutation; strict failure rollback; stale writer conflicts; failed refresh preserving the previous snapshot; stable locks across recreation; and child-process crash injection around commit/migration. Include alias/provenance preservation and caches held by live query views. No storage backend prototype or redesign was implemented in this review.
