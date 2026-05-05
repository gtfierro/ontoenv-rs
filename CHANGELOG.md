# Changelog

All notable changes to this project are documented here. Releases follow [Semantic Versioning](https://semver.org/).

---

## [Unreleased]

### Added
- Progress reporting for `update` command; output suppressed when stderr is not a TTY
- RDF5D: compact string/literal dictionaries, streaming spill policy, workload profiling, and optimized reader/metadata layout

### Changed
- `rocksdb` is now an optional feature flag across all crates (no longer compiled by default)
- Upgraded reqwest 0.12 → 0.13

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
