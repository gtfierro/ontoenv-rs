# Phase 1: API contracts and backend capabilities

Status: first consolidation implementation slice. The approved plan permits
breaking cleanup in unreleased 0.7. Dataset views/copies and dataset results for
unions are required. Backing storage, reader protocol, durability barriers and
transaction-state ownership are unchanged by this slice.

## Raw-store decision

Replace `GraphIO::store() -> &Store` with
`GraphIO::oxigraph_store() -> Result<&Store>`. Unsupported backends return the
existing typed `StoreCapabilityError` by default.

Removing every raw-store operation would also remove useful access for
caller-supplied Oxigraph stores and require replacing their efficient streaming
defaults. The narrow fallible capability keeps that legitimate use explicit
without introducing a capability registry or another backend abstraction.

| Backend | Authoritative raw Oxigraph access | Graph/dataset operations |
| --- | --- | --- |
| Managed snapshot reader/writer | Unsupported; scratch store removed | Snapshot-backed implementations |
| Legacy persistent RDF5D | Unsupported; resident Oxigraph graphs are a lazy cache | Existing lazy/cache-aware implementations |
| Legacy read-only RDF5D | Unsupported, including after a graph has been loaded | Existing RDF5D reads and counts |
| Python `graph_store=` adapter | Unsupported; scratch store removed | Python store callbacks and graph conversions |
| Caller-supplied Oxigraph store | Supported; returns the supplied store | Efficient Oxigraph defaults |
| In-memory Oxigraph backend | Supported; returns the actual memory store | Efficient Oxigraph defaults |

Raw writes bypass OntoEnv's catalog/publication protocol. The caller remains
responsible for external-store reconciliation. Capability availability does not
promise managed transaction semantics for that adapter.

Default GraphIO methods that need Oxigraph now propagate capability errors.
The best-effort default union records the error for each requested graph rather
than reporting a misleading successful empty result. Backends with a different
representation override these operations. Existing streaming Oxigraph and
legacy RDF5D union paths remain intact.

The `ensure_loaded` trait hook is removed: legacy implementations already handle
loading privately in their graph operations. Managed `flush` is explicitly a
backend no-op, including builds with the RocksDB feature. Complete publication
and flushing now belong to the concrete managed candidate path; Phase 4 removes
the optional `GraphIO::publish_environment`, snapshot, refresh and prune hooks.
Use `OntoEnv`/snapshot handles for those lifecycle operations.

### Migrating Rust adapters

- Remove an old `store()` implementation that returned a scratch or partial store.
  Override graph operations required by the adapter instead.
- An adapter whose Oxigraph store contains all of its data can implement
  `oxigraph_store()` and return `Ok(&self.store)`.
- Replace legitimate raw callers with `io.oxigraph_store()?`. Callers that work
  across backends should use `get_graph`, `graph_ids`, `size`, union operations
  or a managed snapshot.
- Move any old `ensure_loaded` behavior into the adapter's graph-read methods.
- Keep external-store access-mode, persistence and reconciliation requirements
  explicit. A complete cache still cannot bypass its file writer's bookkeeping.

## Public surface inventory

The disposition column guides subsequent consolidation slices. It does not
claim that the proposed method removals or query APIs are implemented here.

| Operation family | Current Rust/Python/CLI surface | Disposition |
| --- | --- | --- |
| Existing-environment reads | Rust `open(..., read_only)`, `open_snapshot`, `SnapshotReader`; Python `open(..., read_only)`, `read_snapshot`; CLI read commands | Retain an explicit reader role and immutable selection. Choose one documented acquisition path per use case; remove redundant wrappers in Phase 5. |
| Creation and writing | Rust `create`, `connect`, `init`, `open_snapshot_writer`, `SnapshotWriter`; Python `create`, `connect`, constructor flags; CLI `init`, `add`, `update` | Retain explicit create/open-writer distinctions. Avoid a default query path that silently acquires the sole writer lock. |
| Temporary environments | Rust convenience constructors and `MemoryGraphIO`; Python `temporary=True`, `temporary_snapshot` | Retain temporary behavior; consolidate overlapping construction paths. |
| External-store setup | Rust `*_with_graph_io`, `adopt`, `new_from_store`; Python `graph_store=`, `adopt`, deprecated `init_from_store` | Adapter-specific. Remove obsolete flags/duplicate entry points; preserve supported adapter use through a clear setup API. |
| Source ingestion | Rust/Python add, rename, byte ingestion, updates; CLI add/update/init scanning | Writer operations. Keep overwrite/import-depth/failure semantics explicit. |
| Import queries with fetching | Python `get_dependencies` / `import_dependencies` with `fetch_missing`; Rust source/import pipeline | Separate fetching/writing from pure snapshot queries; snapshot reads do not fetch missing data. |
| Graph reads/copies | Rust `get_graph`/`copy_graph` both return owned Graph; snapshot `graph_view`/`view` borrow query views; Python `get_graph` is read-only and `copy_graph` mutable | Choose paired view/copy vocabulary with honest return types. Current Rust docs now describe materialization accurately. |
| Closure membership/results | Rust `get_closure` returns identifiers; Python `get_closure` returns a flattened view and identifiers, `copy_closure` materializes | Complete snapshot-based queries in Phase 2; share membership/transformation logic. Distinguish membership from graph results. |
| Explicit unions | Rust union APIs return dataset-backed `UnionGraph`; Python `get_union` / `copy_union` primarily return graph results; CLI union exports | Retain graph union and dataset union through explicit result contracts. Avoid changing graph contexts accidentally. |
| Datasets | Python `get_dataset`, `copy_dataset`, `refresh_dataset`, deprecated dataset conversion aliases | Retain dataset view/copy parity with graph operations. Extend dataset scope to explicit unions in Phase 2; evaluate removing rebinding from the preferred API. |
| Streaming and SPARQL/export | Python triple iterators and view queries/serialization; Rust RDF5D scans/views; CLI query/closure/union exports | Keep the existing query engine. Derive iterators from a captured snapshot, preserve ownership and specify whether results count triples or quads and whether duplicates across graph contexts are projected away. |
| Metadata and diagnostics | Ontology listings, aliases, namespaces, dependencies/importers/paths, status, doctor, stats, DOT exports | Snapshot-derived reads, with source/filesystem diagnostics explicitly distinguished. |
| Configuration | Config/overrides, mutable setters, namespace bindings; CLI config | Record which changes publish immediately and which wait for a writer operation. Resolve surprising close/flush boundaries during state consolidation. |
| Refresh and synchronization | Reader `refresh`, source `update`, external `refresh_from_store`, dataset `refresh_dataset` | Keep these meanings separate; place each on its reader, writer or adapter role. Avoid a single ambiguous sync operation. |
| Mutation grouping | Rust `transaction`, `SnapshotUpdate`, internal batch scopes/checkpoints | One public managed grouping contract; consolidate implementation state in Phase 4 while preserving unwind/error behavior. |
| Maintenance | migrate, protocol upgrade, prune, reset/recreation; legacy recover | Maintenance/adapter APIs. Keep migration guarantees and isolate legacy recovery from normal managed reads. |
| Raw backend access | Public `io()` plus mandatory `store()` | Narrow fallible `oxigraph_store()` implemented in this slice. No fake or partial raw-store exposure. |

## Proposed view/copy surface for Phase 2

Use one pair per result shape, deriving every result from the same immutable
snapshot. The proposed names below are for review; only the raw-store capability
change is implemented in this slice.

| Scope/result | Read-only view | Mutable copy |
| --- | --- | --- |
| One graph | `view_graph(iri)` | `copy_graph(iri)` |
| Flattened imports closure | `view_closure(iri, ...)` | `copy_closure(iri, ...)` |
| Flattened explicit graph union | `view_union(iris)` | `copy_union(iris)` |
| Named-graph dataset, whole environment or selected union | `view_dataset(iris=None)` | `copy_dataset(iris=None)` |

Selecting IRIs for a dataset is the dataset form of a graph union, rather than a
separate family of backend-specific union methods. `None` selects the whole
captured catalog; an explicit empty selection selects no graphs. Resolve aliases
and missing graph identifiers against that captured catalog, not a newer head.

Dataset results preserve selected named-graph identities. The same triple in
two named graphs remains two quads. A flattened graph result is a set of triples
without graph contexts. View/copy pairs must agree on the triple or quad set for
the same scope and transform options. Graph-union transforms must not silently
apply to raw dataset unions. Specify default-graph handling and namespace merging
when implementing the result constructors.

Obtain a fresh view to select a newer version. Existing views and iterators keep
their selection after refresh/close. Copy mutations affect the returned object
only. Dataset rebinding is not necessary for this normal workflow.

## Canonical IRI and source conflicts

Managed catalogs retain one registration per canonical IRI and one registration
per source. The import pipeline's `Overwrite::Preserve` reuses the existing
registration when another source declares the same canonical IRI. Explicit
`Overwrite::Allow` replaces its contents/source and preserves aliases. Previously
captured snapshots still retain their old registration and graph data.

The `default`, `latest` and `version` resolution policies select among multiple
catalog registrations only when a temporary/external catalog can represent
them. They do not retain alternative source versions or override the managed
pipeline's preserve/replace choice. Managed builds needing a particular version
must select that source explicitly, or use distinct IRIs for coexisting versions.
Discovery order is not a reproducible source-selection contract.

Phase 1 includes regressions across all three policy settings: preserve rejects
a later source with higher version metadata; explicit replacement accepts a
lower version, keeps one registration, and preserves aliases and old snapshots.

## Validation scope

- Managed writer/reader raw refusal alongside correct graph copies, union quads,
  counts, publication and flush.
- Legacy lazy reader/writer refusal before and after cache warming, alongside
  correct reads and union/count results.
- In-memory and caller-supplied Oxigraph access sharing the actual data with
  GraphIO defaults, including replacement and removal.
- Unsupported default operations returning typed errors or best-effort failures,
  rather than false empty successes.
- Relevant Python custom-store, snapshot and view/copy tests, plus workspace
  compiler/lint checks. Full platform CI remains the integration gate.

No owned-byte/mmap decision, reader-protocol change, durability change or
transaction-state rewrite is included here.

Local validation of this slice: 256 release Rust workspace tests/doctests passed
(eight ignored helpers/doctests); 97 targeted Python tests passed against the
rebuilt installed abi3 wheel (21 existing warnings). All four capability
regressions also pass with `rocksdb` enabled, including managed flush and typed
refusal of an unsupported default flush. Workspace all-target/all-feature release
clippy passes with warnings denied; formatting/diff and Sphinx HTML checks pass.
The five new regressions are included in the focused platform CI invocation.
