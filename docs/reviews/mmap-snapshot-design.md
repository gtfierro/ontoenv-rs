# Immutable RDF5D generations with a transactional manifest

Status: design sketch, not implemented. Builds on `transactional-state.md` and the requested workload: local filesystem, one writer, approximately 100 graphs/10–20 MB, bursty updates, readers explicitly refreshing.

## Initial design choice

Keep **one complete RDF5D store per generation**, rather than immediately splitting into one file per graph. Store generation metadata and the current head in a small transactional database. Every published RDF5D file is immutable.

This preserves the existing whole-store term dictionary, `rdf5d::Snapshot`, scoped `View` indexes, and cross-graph SPARQL behavior. Per-graph files require coordinating independent term dictionaries and blank-node identity during cross-graph queries; that is a separate redesign. At this size, rewriting 10–20 MB per burst is a reasonable first implementation to benchmark.

The manifest implementation can be redb with its experimental multiprocess `SingleWriter` mode, or SQLite WAL. Keep the interface narrow, but build one actual backend first rather than maintaining two implementations prematurely. redb's experimental mode needs a child-process/crash proof of concept before relying on it for production. [redb source](https://docs.rs/redb/latest/src/redb/db.rs.html).

## Disk layout

```text
root/
  .ontoenv.lock                 # stable writer/lifecycle lock; never replaced
  .ontoenv/
    manifest.db
    objects/
      <unique-object-id>.r5tu   # immutable complete graph store
    staging/
      <mutation-id>/            # private preparation; not visible to readers
```

Use unique object IDs initially. Generation numbers are assigned at publication, so an interrupted attempt cannot collide with a reused generation number. A later content-addressed scheme is optional; encoded-byte hashes do not imply canonical RDF equivalence.

The database contains:

```text
environment: { uuid, head_generation, schema_version }
generation[g]: {
  parent_generation,
  recorded_at,
  store_object_id,
  store_length,
  store_checksum,
  rdf5d_format_version,
  catalog_payload,
  persistent_config_payload
}
```

The catalog includes graph existence, ontology/source records, explicit aliases, namespaces and persisted import failures. An empty graph can exist in the catalog even if the codec has no physical directory entry for zero triples; graph APIs must distinguish that from a missing graph. Cache freshness data must distinguish source fetch time from metadata reconciliation time.

## Writer protocol

1. Acquire the stable exclusive writer/lifecycle lock. Only writers, migration and destructive lifecycle operations take it. Readers never take it for their handle lifetime.
2. Open the committed head as an immutable base snapshot. Record its environment UUID and generation.
3. Prepare a private candidate: metadata clone plus explicit graph replacements/deletions. Fetch/parse imports into staging; graph writes never mutate the base snapshot or its mapped file.
4. Validate the complete candidate, including strict imports, aliases, graph existence, and source records. Strict failure discards the candidate. Non-strict mode may publish successful imports with a failure report, as one complete state.
5. Serialize the candidate into a new staging RDF5D file. Untouched graphs come from the base snapshot; replacement graphs come from the overlay, including empty replacements; deletions are omitted. Preserve within-generation term/blank-node semantics.
6. Validate the staged artifact, sync it, move it to its unique immutable object path, and sync the relevant directories. Complete all file durability work before publishing a database reference.
7. Begin a short database write transaction. Check expected UUID/head, insert the generation record and atomically change head. Commit with durable settings.
8. Install the committed candidate as the writer's current snapshot, or lazily reopen its now-published object. Readers can continue using the old generation throughout preparation and publication.

After the database commit, return the committed generation ID. Do not report a generic rollback merely because a subsequent local cache installation fails: the commit is already published. Provide an explicit committed-but-local-refresh-failed outcome if needed, allowing reopening by generation. An uncertain database commit error likewise needs reconciliation against durable head/commit identity before retrying.

`Drop` discards uncommitted staging. It must not serialize or publish changes. Automatic `flush` is not a substitute for commit.

## Crash outcomes

| Interruption | Visible state |
| --- | --- |
| During fetch/parse/serialization | Previous committed head; abandoned staging can be cleaned later. |
| After immutable file publication, before database commit | Previous head; a complete unreferenced object may remain. |
| During database transaction | Database determines whether old or new head committed; reconcile uncertain outcomes. |
| After database commit | New generation, referencing a file made durable before the commit. |

Normal readers never scan staging or select objects by filesystem mtime. They follow only committed manifest references. Corrupt/missing referenced objects are errors; do not silently fall back to a different generation.

## Reader protocol

1. Open the manifest read-only and read head plus its complete generation record in one short read transaction.
2. Open/map exactly the immutable RDF5D object referenced by that record, with bounds/format validation. Bind it and the metadata to one `Arc<EnvironmentSnapshot>`.
3. End the manifest read transaction. The reader retains the snapshot and mapping, without retaining a database transaction or blocking writer publication.
4. All ontology listings, aliases, dependency closures, graphs and queries derive from that snapshot. `snapshot_info()` exposes UUID, generation, parent, commit-record time and reader-open time.
5. Explicit refresh constructs a replacement snapshot completely before replacing the reader's current `Arc`. Existing views/iterators keep their old `Arc`.

Mapping a file does not require decoding every triple block. However, validation can still scan bytes depending on integrity settings; preserve the existing validation policy and benchmark it separately from header parsing and graph decoding. Hash/checksum the candidate during writing where possible; avoid gratuitous full-store reads on every warm open.

## Retention and garbage collection

**First implementation: no automatic generation deletion.** Published objects remain available until an explicit offline prune, which requires all readers closed. This makes arbitrary reader lifetimes correct without adding leases or long database transactions. Expose retained-byte statistics and document this temporary retention policy.

Online GC is a separate feature, not an assumption. It needs a process-safe generation pin whose lifetime includes every live query/view and any lazily opened file. A future protocol can use stable per-generation shared locks and a brief acquisition gate:

- Reader takes the shared acquisition gate, selects a committed generation, acquires its shared generation pin and opens its object, then releases the gate.
- GC takes the exclusive gate, verifies the generation is neither current head nor explicitly retained, and attempts an exclusive generation pin without waiting indefinitely. Busy generations are skipped.
- GC marks removal transactionally and deletes only unreferenced, unpinned objects. Publication/explicit-retention changes must be coordinated with this check so a candidate cannot become newly referenced during deletion.
- Pin lock files are stable and never replaced/unlinked while participants can reference them. OS locks release on process death. Old views retain pins through their owning snapshot.

Do not use timeout-only leases: a sleeping but live reader must not lose its files. Do not rely on Unix unlink behavior as the entire protocol; unopened lazy objects and Windows semantics also matter. Read-only access means no catalog mutation; operational lock files must be provisioned so readers can acquire the required shared locks without creating them.

Reset/recreation must preserve the stable lock and respect pins. Initially require readers closed for physical deletion; a later logical reset can publish an empty generation while retaining old objects.

## API shape

```rust
// Illustrative signatures, not compile-ready declarations.
let mut reader = OntoEnvReader::open(root)?;
let snapshot = reader.snapshot(); // Arc<EnvironmentSnapshot>
let info = snapshot.info();
let graphs = snapshot.ontologies();

let mut writer = OntoEnvWriter::open(root)?;
let mut tx = writer.begin_update()?;
tx.update_sources(options)?;
let commit = tx.commit()?;

reader.refresh()?; // atomic snapshot replacement, no source updates
```

Reader types have no save/add/alias-edit/reconcile methods. A writer transaction owns the candidate; abort and Drop leave the previously committed state visible. Convenience `writer.add(...)` and `writer.update(...)` can wrap one transaction. A committed generation bundles metadata and graphs; there is no separately persisted in-memory environment.

Separate source updates from snapshot refresh and external backend reconciliation. Generic custom backends must declare their snapshot/transaction capabilities; a local manifest alone cannot guarantee consistency with independently changing external storage.

## Mapping onto this repository

- Reuse `rdf5d::Snapshot` / `View` for immutable complete-store reads and lazy indexes.
- Extract file serialization from `PersistentGraphIO` so it writes a specified candidate path without mutating/replacing a committed store.
- Replace `dirty/batch_depth` plus drop-time persistence with a private update overlay and explicit commit.
- Bundle `Environment`, persistent config, dependency graph and RDF5D snapshot into a generation-scoped object. Rebuild derived metadata indexes from that object's catalog.
- Store the existing serde catalog payload directly in the manifest initially; `catalog.r5tu` can become an export artifact rather than a second authority.
- Eliminate pathname stat revisions and independently writable config/catalog files. Derive identity from UUID/generation and cached object validation information.
- Ensure graph reads do not consult mutable source files. Query caches belong to an immutable snapshot; old views must retain ownership rather than accessing a reader's current generation indirectly.
- Audit Python views, raw Oxigraph handles and custom `GraphIO` paths for mutation escape hatches and snapshot lifetimes.

## Incremental implementation

1. Prove the chosen manifest backend across child processes: writer stays open, readers open read-only, old snapshots survive commits, and crashes recover cleanly. Measure its minimum Rust version against this workspace's supported compiler.
2. Build complete immutable generations and reader open/refresh. Initially retain all generations and preserve existing query behavior.
3. Route imports, updates, renames, aliases and config edits through private candidates and one commit path. Fix the reproduced rollback/empty-graph/alias/access-mode bugs as acceptance tests.
4. Add migration and Rust/Python compatibility wrappers. Migration publishes generation zero only after validating old config/catalog/store; interrupted legacy mutations need explicit recovery.
5. Benchmark cold/warm open, first-graph access, multi-graph SPARQL, decoded/index memory across readers, writer peak memory, full-file write latency and retained disk growth.
6. Add online GC if retention measurements justify it. Consider per-graph files only if complete-generation rewrites are actually a bottleneck.

This is a substantial persistence/lifecycle refactor. The graph codec and much of the query engine can stay; the authority boundary, update staging, access modes and snapshot ownership must change together.
