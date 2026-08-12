# Changelog

All notable changes to this project are documented here. Releases follow [Semantic Versioning](https://semver.org/).

---

## [Unreleased]

## [0.6.2] — 2026-08-12

### Added
- `OntoEnv.temporary_snapshot()` (Python) and `OntoEnv::new_temporary()`
  (Rust) create isolated in-memory snapshots that copy both catalog metadata
  and graph contents.

### Changed
- `temporary=True` always creates an empty in-memory environment. Incompatible
  lifecycle-option combinations now fail with a clear error instead of being
  silently ignored.

## [0.6.0] — 2026-07-27

### Added
- `ontoenv recover` rebuilds a persistent environment catalog after
  `CatalogRecoveryError`, using normal environment discovery and removing
  `catalog.pending` only after successful publication.

### Deprecated
- `OntoEnv(..., create_or_use_cached=True)` is now a compatibility-only alias
  for the create-or-reopen lifecycle. It emits `DeprecationWarning`; use
  `OntoEnv.connect(path)` instead. Removal is planned for 0.7.

### Changed (breaking)
- The documented Rust MSRV is now 1.88, matching rdf5d's edition 2024 and the
  resolved dependency floor.
- `OntoEnv.get_graph(uri)` now returns a **read-only store-backed `rdflib.Graph` view** instead of a mutable in-memory copy. Mutating the returned graph raises `ValueError`. Use the new `OntoEnv.copy_graph(uri)` for the previous behavior (mutable in-memory `rdflib.Graph` copy).
- `OntoEnv.get_closure(uri)` now returns a **read-only, zero-copy `ontoenv.ViewGraph`** instead of a materialized mutable `rdflib.Graph`. The materializing behavior moved to the new `OntoEnv.copy_closure(uri)`. A `ViewGraph` does not subclass `rdflib.Graph`; it delegates triple-pattern lookups (`triples`, `subjects`/`predicates`/`objects`), `len`, `in`, and SPARQL `query()` to the Rust backend, reading directly from the rdf5d mmap snapshot. It is read-only — `add`/`addN`/`remove` raise `ValueError`. The view presents a single flattened, de-duplicated graph with the **same triple set as `copy_closure`**: resolved `owl:imports` stripped, ontology declarations collapsed onto the root (adding `root a owl:Ontology` if absent), and SHACL `sh:prefixes`/`sh:declare` consolidated onto the root. Keyword args `remove_owl_imports=True` / `rewrite_sh_prefixes=True` opt out of the respective transform.
- `OntoEnv.get_union(uris)` returns a read-only `ontoenv.ViewGraph` — a **raw** merge across the listed named graphs (no closure transform, no cross-graph de-duplication). Use `copy_union` for a mutable merge.
- `OntoEnv.copy_union(...)` defaults to `rewrite_sh_prefixes=False` and `remove_owl_imports=False` (a raw union, matching `get_union`); pass `True` to opt into the transforms. `copy_closure` defaults both to `True`.
- `OntoEnv.snapshot_as_dataset(backend=..., store=...)` and `OntoEnv.to_rdflib_dataset(mode=...)` are deprecated in favor of `OntoEnv.get_dataset()` (read-only view) and `OntoEnv.copy_dataset()` (mutable copy). The old names still work and emit `DeprecationWarning`.
- `GraphIO::union_graph` now returns `(Dataset, Vec<FailedImport>)` — always best-effort: per-id errors (bad graphname, ensure_loaded failure, mid-graph store iteration error) are recorded in the failures list and the offending id is skipped, but the rest of the union is still assembled. The previous behavior silently dropped failures with no signal. `OntoEnv::get_union_graph` consumes the failures list: in **strict** mode any failure becomes an error; in **non-strict** mode the partial union is returned with `UnionGraph.failed_imports` populated so the caller knows what's missing.
- Catalog adoption and recovery now require a stable, fully readable backend
  snapshot instead of publishing partial metadata when a graph read fails.

### Added
- `OntoEnv.recover(path, graph_store=None)` rebuilds a catalog after an
  interrupted mutation and removes the recovery marker only after the new
  catalog is published.
- Python `UnresolvedImportError` distinguishes an unresolved `owl:imports`
  target passed to `copy_graph` from other `ValueError` failures.
- Python `OntoEnv.update` now accepts an optional source location and always
  replaces that source's stored graph while following its imports.
  `update(force=True)` replaces `update(all=True)`; `all=` remains as a
  deprecated compatibility alias.
- An authoritative RDF5D metadata catalog at `.ontoenv/catalog.r5tu`, with
  graph-free warm opens, automatic legacy JSON migration, backend drift
  detection, recovery markers, explicit `create`/`open`/`adopt` lifecycle
  APIs, a state-driven `connect(sync=...)` entry point, and
  incremental/targeted/full graph-store synchronization reports.
- Durable RDF5D snapshot replacement using unique same-directory temporary
  files, file and directory synchronization, and atomic publication.
- `OntoEnv.copy_graph(uri) -> rdflib.Graph` — materialize a mutable in-memory copy of a single ontology.
- Pythonic container/context-manager protocols on `OntoEnv`: `len(env)`, `uri in env`, `env[uri]` (shorthand for `get_graph`), `for name in env` (iterates ontology URIs), and `with OntoEnv(...) as env:` (calls `close()` on exit). `bool(env)` is always `True` — use `env is None` to detect absence.
- `OntoEnv.copy_closure(uri, graph=None, rewrite_sh_prefixes=True, remove_owl_imports=True, recursion_depth=-1) -> (Graph, list[str])` — materialize the flattened imports closure into a mutable `rdflib.Graph` (the view-returning `get_closure`'s mutable counterpart).
- `OntoEnv.iter_triples(uri) -> Iterator[(s, p, o)]` and `OntoEnv.iter_closure_triples(uri, recursion_depth=-1) -> Iterator[(s, p, o)]` — streaming triples as rdflib terms, skipping the rdflib `Graph` wrapper. Closure iteration is not de-duplicated.
- `ontoenv.ViewGraph` — read-only, non-`rdflib.Graph` view returned by `get_closure` and `get_union`. Exposes `triples`, `subjects`/`predicates`/`objects`, `query` (SPARQL scoped to the view), `len`, `in`, `serialize`, and namespace bindings; mutation raises `ValueError`.
- `_RdfLibStoreBackend` scoped methods: `iter_triples_scoped`, `triples_scoped`, `subjects_scoped`, `predicates_scoped`, `objects_scoped`, and `query_scoped` — the Rust primitives `ViewGraph` delegates to, scoped to a list of named graphs.
- Internal `OntoEnv.get_graph(uri)` Dataset cache: subsequent `get_graph` calls reuse the underlying store; mutating methods (`add`, `add_no_imports`, `update`, `flush`) invalidate it.
- `OntoEnv.refresh_dataset(dataset)` method — re-snapshot the env into an existing `OntoEnvStore`-backed Dataset. Replaces the top-level `refresh_dataset_from_env(dataset, env)` helper.
- `Environment::get_ontology_by_id(&GraphIdentifier) -> Option<&Ontology>` — direct lookup that skips the configured `ResolutionPolicy`.
- rdf5d: `closure` module (`rdf5d::ClosurePatch`, `ClosureTripleIds`, `ClosureSparqlView`) — zero-copy closure semantics over a `Snapshot`. A `ClosurePatch` precomputes the closure transform (imports stripping, ontology-declaration collapse, SHACL-prefix consolidation) as removals (a `keep` predicate) plus a small additions "patch graph", all in on-disk term-id space; the iterator and SPARQL views apply it lazily to present a single flattened, de-duplicated graph. Used by `OntoEnv.get_closure`.
- rdf5d: `Snapshot::build_indexes()` and `R5tuFile::build_term_index()` — eagerly build all four permutation indexes (and the reverse term-id index) at bind time.
- `GraphIO::ensure_loaded(&GraphIdentifier) -> Result<()>` trait hook for persistent backends to lazily load named graphs into the in-memory store. Default impl is a no-op.
- `FailedImport::ontology()` and `FailedImport::error()` expose union failures
  to Rust callers without parsing display strings.

### Removed
- Top-level re-exports `ontoenv.dataset_from_env` and `ontoenv.refresh_dataset_from_env` — use `env.get_dataset(...)` and `env.refresh_dataset(...)` instead. The functions still exist in `ontoenv.rdflib_store` as the underlying implementation.

### Fixed
- `ontoenv dep-graph` no longer defines a short `-o` for `--output`, which
  collided with the global `-o/--offline` flag and made `ontoenv dep-graph
  --help` panic on debug builds. Use the long `--output` form.
- Python reopen paths now distinguish omitted configuration from explicit
  values for strict/offline/name-validation/cache settings, resolution policy,
  cache TTL, search paths, and include/exclude filters. Explicit `False`,
  `"default"`, and empty lists are honored. Writable connections persist
  overrides; read-only connections keep them session-local; no reopen path
  implicitly scans or re-ingests graph data.
- Runtime configuration setters now update the active graph backend and
  resolution policy. Python now exposes the documented cache-mode and
  remote-cache-TTL getters and setters, and `require_ontology_names` controls
  ontology-declaration validation independently of strict mode.
- Non-strict `import_dependencies(..., fetch_missing=True)` and
  `get_dependencies(..., fetch_missing=True)` now commit tolerated unresolved
  imports without leaving `catalog.pending`, and every attempted unresolved
  target retained in the current environment state is classified as
  `UnresolvedImportError` by `copy_graph`, including targets originating in
  best-effort fetches for transient caller graphs.
- Successful non-strict `add`/`add_from_bytes` ingestion with unresolved
  imports no longer leaves a recovery marker; the partial environment remains
  reopenable.
- Writable persistent stores enumerate graph IDs from their lazy RDF5D
  directory, allowing non-empty 0.5 catalogs to migrate without a false
  backend-mismatch error.
- `ViewGraph.triples` scoped-pattern branch: `triples_scoped` returns `(triple, contexts)` rows, but the code yielded the whole row — now unpacks to the bare triple.
- `OntoEnv.add(..., rename=...)` rename test expectation: the minimal `<old> a owl:Ontology .` fixture contains exactly one triple, and `rename_ontology_iri_graph` rewrites the subject to yield one triple; the `test_add_with_rename_overrides_iri` assertion of `len == 2` ("type + declare") was a stale expectation with no matching fixture, corrected to `len == 1`.
- `add_ids_to_dependency_graph` is now transactional with respect to the in-memory env state: a mid-traversal failure (e.g. a strict-mode unresolved import) no longer leaves `env`, `dependency_graph`, `dependency_graph_index`, and `failed_resolutions` desynced from each other.
- Dependency-graph construction now resolves imports by `GraphIdentifier` instead of going through `ResolutionPolicy`, so the graph reflects the exact ontology being added rather than whatever the policy maps the name to.
- Python maps lifecycle errors by their Rust types rather than matching error
  message text.

### Performance
- `get_closure` view reads are served directly from the rdf5d mmap snapshot with no materialization. Permutation indexes are built eagerly (in parallel) when the snapshot is bound, and each `_RdfLibStoreBackend` keeps a persistent term-id → rdflib-term cache shared across `triples()` calls, so repeated scans skip rdflib `URIRef`/`Literal` construction. Full closure iteration on the Brick closure runs in ~30 ms warm.
- `get_closure` BFS-walks the pre-built `dependency_graph` via `NodeIndex` instead of resolving each import by name on every step. A new `dependency_graph_index` map is kept in sync with the graph.
- `Environment::get_ontology` short-circuits exact-id hits and skips the per-call `Vec<&Ontology>` policy fallback.
- `GraphIO::union_graph` streams quads from the store directly into the target `Dataset`, dropping the intermediate per-id `Graph` allocation.
- `get_union_graph` and `get_namespaces` borrow ontologies from `env.ontologies()` instead of cloning each one through `get_ontology`.

---

## [0.5.5]

### Added
- `OntoEnv.as_dataset(backend="auto", store=None)` — return a read-only `rdflib.Dataset` view of the environment. `backend="rdf5d"` is a zero-copy mmap-backed view over the persistent `.ontoenv/store.r5tu` snapshot; `backend="copy"` materializes an in-memory copy; `backend="auto"` picks rdf5d when the snapshot file exists and copy otherwise.
- New `ontoenv.OntoEnvStore` rdflib `Store` (also registered as the rdflib plugin `"ontoenv"`) that serves SPARQL through the Rust backend, with `dataset_from_env` / `refresh_dataset_from_env` helpers in `ontoenv.rdflib_store`.
- rdf5d: SPARQL backend (`rdf5d::SparqlDatasetView`) and a Brick benchmark comparing it to Oxigraph + RocksDB; rdf5d wins on the tested patterns (≈18% faster on bound-graph queries, ≈2× faster on full scans).

### Changed
- `Rdf5dSnapshot::open` is now O(graphs) rather than O(triples) — per-logical-graph unique-triple counts are computed lazily via `OnceLock`, with a single-gid fast path that trusts the GDIR `n_triples` directly. Reverse term lookup (`find_term_id`) memoizes against `R5tuFile::find_decoded_term` so repeated SPARQL bindings of the same IRI stop re-scanning the term table.
- Copy-fallback Dataset construction (the `backend="copy"` / `backend="auto"` fallback path) builds the materialized `OxDataset` directly from the inner Rust `OntoEnv`, dropping the previous round-trip through an intermediate `rdflib.Dataset`.

### Deprecated
- `OntoEnv.to_rdflib_dataset(mode=...)` — use `OntoEnv.as_dataset(backend=..., store=...)` instead. The old method still works (and forwards to the new one) but now emits `DeprecationWarning`. The new method renames the parameter (`mode` → `backend`) and accepts an optional `store=` to rebind an existing rdflib `Store`; error messages now reference `backend=` accordingly.

---

## [0.5.4]

### Added
- Progress reporting for `update` command; output suppressed when stderr is not a TTY
- RDF5D: compact string/literal dictionaries, streaming spill policy, workload profiling, and optimized reader/metadata layout
- GitHub badge in README; rdf5d architecture documentation

### Changed
- RocksDB is no longer compiled by default. OntoEnv uses an in-memory Oxigraph store backed by the custom RDF5D on-disk format, so the heavyweight RocksDB C++ dependency was unnecessary. It is now opt-in via `--features rocksdb` across all crates (`ontoenv`, `ontoenv-cli`, and the Python bindings). This significantly reduces compile times and binary size for the common case. `Store::flush()` is gated behind the same feature flag since that method only exists when RocksDB is compiled in.
- Upgraded reqwest 0.12 → 0.13
- Internal `lib/src` helpers extracted and dead code removed
- Removed deprecated `tempdir` dependency

### Fixed
- `ontoenv add` now correctly handles JSON-LD files served with a `text/plain` content-type header (e.g. GitHub raw URLs); URL extension and content sniffing are used when content-type is generic
- `ext_to_rdf_format` now maps `.jsonld`, `.json`, `.rdf`, `.owl`, and `.nq` extensions for local file loading
- Format fallback in `load_staging_store_from_bytes` now cycles through all supported formats (NQuads, TriG, JSON-LD) instead of only Turtle/RDF-XML/N-Triples
- All `cargo clippy -D warnings` errors resolved across the workspace

---

## [0.5.3] — 2026-04-03

### Added
- `list_closure` and `missing_imports` now accept a transient `rdflib.Graph` in addition to ontology IRIs
- `graph_store`: new `init_from_store` and `refresh_from_store` constructors

### Changed
- Updated `.pyi` stub with all recent API additions
- Updated GitHub Actions versions

---

## [0.5.2] — 2026-04-02

### Added
- `missing_imports` method in Python API to list unresolved ontology imports
- Three-level import chain test for `missing_imports`

### Changed
- Improved documentation

---

## [0.5.1] — 2026-03-04

### Fixed
- Linux wheel build

### Changed
- Updated license year to 2025

---

## [0.5.0] — 2026-03-03

### Added
- In-memory `rdflib.Graph` objects can now be passed directly to `OntoEnv.add` in Python
- External Python graph store protocol support (duck-typed; no ABC required)
- `namespaces` CLI command and Python/Rust API method
- sh:prefix conflict detection in `rewrite_sh_prefixes`
- ontology include/exclude regex and glob filters (`Config.include`/`Config.exclude`)
- Content hash-based caching to avoid redundant re-parses
- Sphinx documentation with GitHub Pages deployment
- `llms.txt` for LLM-friendly docs
- `oxrdflib` integration

### Changed
- Lazy loading of graphs from RDF5D on first access
- `get_dependencies_graph` renamed to `get_dependencies`
- SHACL prefix rewrite now correctly targets the root ontology
- Python build switched to `abi3` wheels (Python 3.12 default)
- Search directories made explicit in config

### Fixed
- Windows cross-platform path and file URI handling
- CI flakes: mtime sleep guards, Windows file IRIs, locked-file skipping
- `import_graph` depth/cycle handling and QUDT URI updates

---

## [0.4.0] — 2025-11-07

### Added
- **RDF5D custom storage format** (`.r5tu`) replaces SQLite-backed Oxigraph on-disk store; zstd-compressed, CRC-verified, with lazy graph loading
- Interprocess read/write locking via `fs2` (exclusive writer, shared readers)
- Parallel remote ontology fetching via staged ingestion
- New `fetch` module with layered format detection, content sniffing, Link header following, and extension candidate fallbacks
- `use_cached_ontologies` option to skip unchanged ontologies
- `get_dependencies` / `get_dependencies_graph` method (Rust + Python)
- `--all` flag for `update` to force-reload all ontologies
- `ONTOENV_LOG` environment variable for log control
- Concurrency tests (Python and Rust)
- `new_online` constructor as the default for Python

### Changed
- Upgraded to oxigraph 0.5
- `update` command gains `--all` flag; `update_all` alias added
- Config drops `ConfigBuilder` from Python API; flags passed directly to `OntoEnv`
- `import_graph` merges full closure with SHACL rewrite
- Namespace prefix map built at ontology init time
- RDF5D localized into this repo as a workspace crate

### Fixed
- Failed ontology resolutions tracked to avoid redundant retries
- Correct graph-name handling for oxigraph queries
- File URI generation and angle bracket stripping in IRIs

---

## [0.3.0] — 2025-07-24

### Added
- `why` command and `importers` method (replaces `get_dependents`) — explains why an ontology is in the environment
- `missing_imports` method to list unresolved `owl:imports`
- `list` subcommand for locations, ontologies, and missing imports
- `config` subcommand (replaces `set`) with `get`, `unset`, `add`, `remove`, `list` operations
- `add_no_imports` flag to load an ontology without following its `owl:imports`
- `recursion_depth` parameter for `get_closure` and `import_dependencies`
- Recursive `.ontoenv` directory search from the current working directory
- Namespace prefix extraction and utility functions
- `ExternalStoreGraphIO` for integrating with other Oxigraph-based packages
- Comprehensive Python `unittest` suite

### Changed
- `refresh` command renamed to `update`
- `get-closure` CLI subcommand renamed to `closure`
- `Config` builder pattern replaces direct struct construction
- `import_dependencies` returns a list of URIs and modifies graph in-place
- `add` auto-detects whether the argument is a URL or file path

### Fixed
- Namespace map deserialization robustness
- Self-import filtering to prevent recursion
- `no_search` respected when loading config from file
- Graph content compared (not just mtime) to detect updates

---

## [0.2.1] — 2025-06-06

### Fixed
- Improved detection of changed files

---

## [0.2.0] — 2025-05-07

### Added
- New `GraphIO` trait abstraction (`PersistentGraphIO`, `MemoryGraphIO`, `ReadOnlyPersistentGraphIO`)
- `UnionGraph` struct returned from `get_union_graph`
- `flush` method for explicit store writes
- `size` stats reporting
- `io_type` accessor on `GraphIO`
- Serialization of `Environment` struct
- Python `flush` binding

### Changed
- `OntoEnv::new` made private; use named constructors
- `search_directories` made a positional CLI argument
- Poetry replaced with `uv` for Python tooling
- Temporary environments improved; `--force` flag for reset

### Fixed
- Offline retrieval error propagation
- Store initialization and path handling
- Stat report accuracy

---

## [0.1.10] — 2025-03-19

### Added
- `get_dependents` method
- Type hints stub file (`.pyi`) for `Config` and `OntoEnv`
- JSON-based ontology URI/file config (`fetch` subcommand)
- `no_search` flag to disable directory walking
- Read-only mode for `OntoEnv`
- `read_format` fallback logic for ambiguous RDF inputs
- Accept `text/turtle` content-type header

### Changed
- Bulk loading of graphs for performance
- Mutex lock scope reduced in `get_graph` to lower deadlock risk

### Fixed
- Store re-opened unnecessarily on repeated calls — fixed by caching
- Mutex unlocking on drop

---

## [0.1.9] — 2024-08-28

### Added
- `list_closure` method
- `status` and `version` CLI commands
- `rdflib` graph conversion method (`to_rdflib`)
- Git hash embedded in CLI binary via `build.rs`

### Changed
- Build system improvements (musl, zig cross-compilation)

---

## [0.1.8] — 2024-06-15

### Added
- Read-only mode fallback in Python
- More test coverage

### Fixed
- Strict mode now respected throughout
- Resolution of ontology locations
- Python state persistence across calls

---

## [0.1.6] — 2024-04-29

### Changed
- Switched from OpenSSL to `rustls` (no system SSL dependency)
- Cross-platform build improvements (aarch64, x86 macOS, Linux musl)

---

## [0.1.5] — 2024-04-28

### Added
- `--recreate` flag for `init` to force reinitialize an existing environment

---

## [0.1.4] — 2024-04-26

### Added
- `import_graph` method (Python)
- Graph transforms (SHACL prefix rewriting, import removal)
- README

### Fixed
- URL handling and path normalization
- Detection of removed files

---

## [0.1.2] — 2024-04-13

Initial release.

### Features
- Core ontology environment management: discover, load, and resolve `owl:imports` transitively
- CLI: `init`, `add`, `closure`, `get`, `dump`, `status`
- Python bindings via PyO3/maturin
- Offline mode
- Strict mode (require `owl:Ontology` declarations)
- Directory walking with glob patterns
- CI/CD for Linux, macOS, and Windows wheels
