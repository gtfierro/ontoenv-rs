# OntoEnv

[![crates.io](https://img.shields.io/crates/v/ontoenv.svg)](https://crates.io/crates/ontoenv)
[![PyPI](https://img.shields.io/pypi/v/ontoenv.svg)](https://pypi.org/project/ontoenv/)
[![docs.rs](https://docs.rs/ontoenv/badge.svg)](https://docs.rs/ontoenv/latest/ontoenv/)
[![License](https://img.shields.io/crates/l/ontoenv.svg)](LICENSE)

`ontoenv` is a lightweight environment manager for RDF ontologies and their imports. It helps you:

- Discover ontologies locally and on the web
- Resolve and materialize `owl:imports` closures
- Query and export graphs

**Components:**
| | Install | Docs |
|---|---|---|
| CLI | `cargo install --locked ontoenv-cli` or `pip install ontoenv` | This README |
| Python library | `pip install ontoenv` | [Python API](#python-api-ontoenv) |
| Rust library | `cargo add ontoenv` | [docs.rs](https://docs.rs/ontoenv/latest/ontoenv/) |

---

## Quick Start

```bash
# Install the CLI
cargo install --locked ontoenv-cli

# Initialize an environment in a directory containing ontology files
ontoenv init ./ontologies

# Compute the imports closure of an ontology and write it to a file
ontoenv closure http://example.org/ont/MyOntology result.ttl
```

Or from Python:

```python
from ontoenv import OntoEnv

env = OntoEnv(search_directories=["./ontologies"], strict=False, temporary=True)

g, closure_names = env.copy_closure("http://example.org/ont/MyOntology")
print(f"Closure has {len(g)} triples")
```

---

## How It Works

`ontoenv` looks for patterns like this in local ontology files:

```turtle
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <urn:my_ontology/> .

<urn:my_ontology> a owl:Ontology ;
    owl:imports <https://brickschema.org/schema/1.4/Brick>,
                <http://qudt.org/2.1/vocab/quantitykind> .
```

When initialized, `ontoenv` searches configured directories for ontology declarations, identifies their `owl:imports`, and recursively pulls in dependencies. The central operation is computing the *imports closure*: the set of all ontologies transitively required by a given root, optionally merged into a single flat graph.

In a persistent environment, OntoEnv saves both the graph content and the
smaller collection of ontology names, imports, aliases, source locations, and
namespaces needed to navigate it. Later connections can restore dependency
lookups from that saved information without rereading every graph. The
environment also coordinates readers and writers so a CLI process, script, or
long-running service sees a consistent saved state.

Discovery remains configurable: applications can decide which local files and
remote IRIs are eligible, work offline, and control when cached remote
ontologies should be refreshed.

### Canonical IRIs and Source URLs

Ontologies fetched from a URL often declare a different (usually versioned) ontology IRI inside the file. `ontoenv` remembers that relationship: when an ontology is added, the source URL is recorded and, if its declared name differs, an alias is created from the normalized URL to the canonical ontology IRI. Future `owl:imports` referencing the versionless URL reuse the already-downloaded ontology instead of re-fetching it.

---

## CLI

### Installation

- `cargo install --locked ontoenv-cli` — from crates.io
- `cargo install --path cli --locked` — from a local checkout
- `pip install ontoenv` — alongside Python bindings
- [Download a binary](https://github.com/gtfierro/ontoenv-rs/releases) from the Releases tab

### Local State

| Path | Purpose |
|---|---|
| `.ontoenv/` | Environment directory |
| `.ontoenv/store.r5tu` | RDF5D persistent store |
| `.ontoenv/catalog.r5tu` | Authoritative, versioned environment metadata catalog |
| `.ontoenv/catalog.pending` | Recovery marker for an interrupted graph/catalog mutation |
| `.ontoenv/store.lock` | File lock (single writer, shared readers) |

Warm opens read only `ontoenv.json`, `catalog.r5tu`, and O(1) backend state when
available. They do not materialize ontology graphs. Legacy `environment.json`
metadata is migrated automatically when its graph IDs match the backend;
legacy JSON files are retained but ignored after migration.

Set `ONTOENV_DIR` to override the environment location. Logging is controlled via `ONTOENV_LOG` or `RUST_LOG`.

### Commands

#### `init`

Initialize an `ontoenv` workspace. Must be run once per environment; all other commands auto-discover the nearest `.ontoenv/` by walking up from the current directory.

```
Usage: ontoenv init [OPTIONS] [LOCATION]...

Arguments:
  [LOCATION]...  Directories to search for ontologies. If omitted, no
                 directories are scanned; add ontologies manually or
                 rerun init with explicit paths.

Options:
      --overwrite                       Overwrite an existing environment
  -r, --require-ontology-names          Raise an error if multiple ontologies share the same name
  -s, --strict                          Raise an error if an import is not found
  -o, --offline                         Do not fetch ontologies from the web
  -p, --policy <POLICY>                 Resolution policy: 'default', 'latest', or 'version' [default: default]
  -i, --includes <INCLUDES>...          Glob patterns to include [default: *.ttl *.xml *.n3]
  -e, --excludes <EXCLUDES>...          Glob patterns to exclude
      --include-ontology, --io <REGEX>  Regex patterns of ontology IRIs to include
      --exclude-ontology, --eo <REGEX>  Regex patterns of ontology IRIs to exclude
  -h, --help                            Print help
```

**Examples:**
```bash
ontoenv init .                                       # scan current directory
ontoenv init ./ontologies ./models                   # scan multiple directories
ontoenv init                                         # empty environment, add ontologies manually
ontoenv init --overwrite --offline ./ontologies      # rebuild from scratch, offline
```

Offline mode is particularly useful when you want to limit which ontologies are loaded: download the ones you want, then enable `--offline` to prevent any further network access.

#### `update`

Refresh the environment based on file timestamps and configuration.

```bash
ontoenv update        # refresh only changed/added files
ontoenv update --all  # rebuild the in-memory view from all sources
```

To change search paths or flags, re-run `ontoenv init`.

#### `closure`

Compute and write the imports closure (union of a graph and its transitive imports).

```bash
ontoenv closure http://example.org/ont/MyOntology                        # writes output.ttl
ontoenv closure http://example.org/ont/MyOntology result.ttl             # writes result.ttl
ontoenv closure http://example.org/ont/MyOntology --no-rewrite-sh-prefixes
ontoenv closure http://example.org/ont/MyOntology --keep-owl-imports
```

By default, SHACL `sh:prefixes` references are rewritten onto the root node and `owl:imports` triples are removed from the output.

#### `get`

Retrieve a single ontology graph from the environment.

```bash
ontoenv get http://example.org/ont/MyOntology                    # Turtle to STDOUT
ontoenv get http://example.org/ont/MyOntology --format jsonld    # JSON-LD to STDOUT
ontoenv get http://example.org/ont/MyOntology --output my.ttl    # write to file

# Disambiguate when multiple sources share the same IRI
ontoenv get http://example.org/ont/MyOntology --location ./ontologies/MyOntology-1.4.ttl
```

Supported formats: `turtle` (default), `ntriples`, `rdfxml`, `jsonld`.

#### Other commands

| Command | Description |
|---|---|
| `ontoenv status [--json]` | Human-friendly environment status |
| `ontoenv dump` | Show ontologies, imports, sizes, and metadata |
| `ontoenv list ontologies [--json]` | List ontology names in the environment |
| `ontoenv list missing [--json]` | List imports not found in the environment |
| `ontoenv dep-graph` | Export a Graphviz import dependency graph (PDF); requires Graphviz |
| `ontoenv why <IRI>... [--json]` | Show which ontologies import the given IRI, as path(s) |

---

## Python API (`ontoenv`)

### Installation

```bash
pip install ontoenv   # Python 3.11+; prebuilt wheels for common platforms
```

Building from source requires a Rust toolchain (MSRV 1.70).

### Basic Usage

```python
import tempfile
from pathlib import Path
from ontoenv import OntoEnv

with tempfile.TemporaryDirectory() as temp_dir:
    root = Path(temp_dir)

    (root / "ontology_a.ttl").write_text("""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://example.com/ontology_a> a owl:Ontology .
""")
    (root / "ontology_b.ttl").write_text("""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://example.com/ontology_b> a owl:Ontology ;
    owl:imports <http://example.com/ontology_a> .
""")

    env = OntoEnv(search_directories=[str(root)], strict=False, offline=True, temporary=True)

    print("Ontologies found:", env.get_ontology_names())

    g, closure_names = env.copy_closure("http://example.com/ontology_b")
    print(f"Closure of ontology_b has {len(g)} triples")  # → 2
```

### Persistent Lifecycle

For most persistent applications, use ``connect``:

```python
from ontoenv import OntoEnv

# First run: create the environment directory, save its settings, and
# initialize graph storage plus an empty ontology index.
# Later runs: reuse that index without rereading every RDF triple.
env = OntoEnv.connect("./my-env")
env.add("./ontologies/site.ttl")

# Keep the object in application state and close it during shutdown.
application_state.ontoenv = env
application_state.ontoenv.close()
```

With ``graph_store=store``, the first connection instead reads graphs already
in that store and records their ontology names, imports, aliases, locations,
namespaces, and hashes. Later connections reuse and synchronize that saved
index.

Use the lower-level methods when you want strict lifecycle control:

```python
OntoEnv.create("./new-env")  # set up empty state; fail if it already exists
OntoEnv.open("./existing-env", read_only=True)  # load only; never create or sync
OntoEnv.adopt("./adopted-env", graph_store)  # index every existing store graph once
```

``connect(sync="auto")`` never silently falls back to a full scan after drift:
it incrementally reconciles stores with graph revisions and otherwise asks for
an explicit ``sync="full"``. It does not refresh ontology files or URLs; call
``env.update()`` after connecting to refresh changed sources and their
imports.

### Constructor

```python
OntoEnv(
    path=None,
    recreate=False,
    create_or_use_cached=False,
    read_only=False,
    search_directories=None,   # pass ["."] to scan immediately; None skips discovery
    require_ontology_names=False,
    strict=False,
    offline=False,
    use_cached_ontologies=False,
    resolution_policy="default",
    root=".",
    includes=None,             # gitignore-style globs; bare dirs match everything under them
    excludes=None,
    include_ontologies=None,   # regex filters on ontology IRIs; excludes run after includes
    exclude_ontologies=None,
    temporary=False,           # in-memory only, no .ontoenv/
    remote_cache_ttl_secs=None,  # max age before re-fetching a remote ontology (default: 86400)
)
```

**Environment modes:**
- `temporary=True` — in-memory only, no `.ontoenv/`
- `OntoEnv.connect(path, sync="auto")` — persistent connection and graph-store synchronization; call `update()` separately for source files and URLs
- `OntoEnv.create(path)` — create a persistent empty environment
- `OntoEnv.open(path, read_only=False)` — require and load an existing catalog
- `OntoEnv.adopt(path, graph_store)` — scan an existing store and create its catalog
- `recreate=True` — explicitly create (or overwrite) at `path`
- `create_or_use_cached=True` — bootstrap a new environment if none is found, otherwise reuse existing
- Default — walk up from `path` (or `root`) to find an existing `.ontoenv/`; raise `FileNotFoundError` if not found

### Dependency Resolution Methods

| Method | Root identified by | Mutates input? | Returns |
|---|---|---|---|
| `get_closure(name, ...)` | IRI string | No | `(ViewGraph, list[str])` read-only view |
| `copy_closure(name, ...)` | IRI string | No | `(Graph, list[str])` mutable copy |
| `import_graph(destination_graph, name, ...)` | IRI string | Yes (required) | `None` |
| `import_dependencies(graph, ...)` | `owl:imports` in caller's graph | Yes | `list[str]` |
| `get_dependencies(graph, ...)` | `owl:imports` in caller's graph | No | `(Graph, list[str])` |
| `list_closure(name, ...)` | IRI string | — | `list[str]` (IRIs only) |

**`get_closure(uri, recursion_depth=-1, remove_owl_imports=True, rewrite_sh_prefixes=True)`**
Return a read-only `ViewGraph` over the full closure by IRI. Persistent local environments serve it directly from the rdf5d mmap snapshot without materializing the closure; temporary and custom-store environments build a normalized in-memory read snapshot. Both paths present a single flattened, de-duplicated graph with the **same triple set as `copy_closure`** (resolved `owl:imports` stripped, ontology declarations collapsed onto `uri`, SHACL `sh:prefixes`/`sh:declare` consolidated). Pass `remove_owl_imports=False` / `rewrite_sh_prefixes=False` to opt out of those transforms. `ViewGraph` is not an `rdflib.Graph` subclass — it delegates `triples`, `subjects`/`predicates`/`objects`, `len`, `in`, and `query()` to the Rust backend; mutation raises `ValueError`.

**`copy_closure(uri, graph=None, rewrite_sh_prefixes=True, remove_owl_imports=True, recursion_depth=-1)`**
Copy the full closure by IRI into a mutable graph. `sh:prefixes` blocks are consolidated onto `uri` and `owl:imports` removed by default. Returns `(merged_graph, closure_iris)`. Use when you want a self-contained graph for reasoning, exchange, or export.

**`import_dependencies(graph, fetch_missing=False)`**
Mutates the provided graph in-place. Reads its `owl:imports` statements, resolves each one transitively, merges all closure triples into the same graph, removes `owl:imports`, and rewrites `sh:prefixes` onto the graph's root. Returns `list[str]` of imported IRIs.

**`get_dependencies(graph, graph_name=None, fetch_missing=False)`**
Same closure as `import_dependencies` but never modifies the original graph. Returns a new graph. Without `graph_name`, each ontology retains its own `owl:Ontology` declaration. With `graph_name`, all declarations are collapsed onto that single IRI and `sh:prefixes` are rewritten onto it. Returns `(deps_graph, list[str])`.

### Other Methods

| Method | Description |
|---|---|
| `update(location=None, force=False)` | Refresh changed sources, or one named source, together with their imports |
| `add(location, fetch_imports=True) -> str` | Add ontology by file path, URL, or `rdflib.Graph`; returns IRI |
| `add_no_imports(location) -> str` | Same as `add`, but skips import traversal |
| `get_graph(name) -> Graph` | Read-only store-backed view of a single ontology (no closure expansion). Mutation raises `ValueError` |
| `copy_graph(name, graph=None) -> Graph` | Mutable in-memory copy of a single ontology |
| `get_closure(name, ...) -> (ViewGraph, list[str])` | Read-only closure view; mmap-backed for persistent local envs, normalized in memory otherwise; same triple set as `copy_closure` |
| `copy_closure(name, graph=None, ...) -> (Graph, list[str])` | Mutable in-memory copy of the imports closure |
| `iter_triples(name)` / `iter_closure_triples(name, recursion_depth=-1)` | Streaming triple iterators that skip the rdflib `Graph` wrapper |
| `get_ontology(name)` | Inspect metadata: imports list, version, namespace map, last-updated |
| `get_importers(name) -> list[str]` | Reverse dependency lookup |
| `get_namespaces(name, include_closure=False)` | Aggregated prefix-to-IRI mappings |
| `missing_imports(uri=None) -> list[str]` | List unresolvable `owl:imports` IRIs (see below) |
| `get_dataset() -> rdflib.Dataset` | Read-only Dataset view of the env |
| `copy_dataset(dataset=None) -> rdflib.Dataset` | Mutable in-memory copy of the env |
| `store_path() -> str \| None` | Path to `.ontoenv/`, or `None` for temporary environments |
| `close()` | Persist (if applicable) and release resources |
| `len(env)` | Number of ontologies in the environment |
| `uri in env` | True if `uri` resolves to a known ontology |
| `env[uri]` | Shorthand for `get_graph(uri)` |
| `iter(env)` | Iterate over ontology URIs |
| `with OntoEnv(...) as env:` | Context manager — calls `close()` on exit |

**`missing_imports(uri=None) -> list[str]`**
Returns a list of `owl:imports` IRIs that could not be resolved in the environment.

- `uri=None` — scans every ontology in the environment and returns the de-duplicated union of all unresolvable imports.
- `uri="http://..."` — walks the full transitive `owl:imports` closure of the given ontology and returns every import IRI that cannot be found, including those declared by transitively-imported ontologies.

```python
# All missing imports across the whole environment
missing = env.missing_imports()

# Missing imports reachable from a specific ontology
missing = env.missing_imports("http://example.org/ont/MyOntology")
```

---

## Rust Library

[![docs.rs](https://docs.rs/ontoenv/badge.svg)](https://docs.rs/ontoenv/latest/ontoenv/)

```toml
[dependencies]
ontoenv = "0.5"
```

Requires Rust 1.70+.

### Basic Usage

```rust
use ontoenv::config::Config;
use ontoenv::api::{OntoEnv, ResolveTarget};
use ontoenv::ToUriString;
use oxigraph::model::NamedNode;
use std::path::PathBuf;
use std::fs;
use std::io::Write;
use std::collections::HashSet;

# fn main() -> anyhow::Result<()> {
let test_dir = PathBuf::from("target/doc_test_temp_readme");
if test_dir.exists() { fs::remove_dir_all(&test_dir)?; }
fs::create_dir_all(&test_dir)?;
let root = test_dir.canonicalize()?;

let mut file_a = fs::File::create(root.join("ontology_a.ttl"))?;
writeln!(file_a, r#"
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://example.com/ontology_a> a owl:Ontology .
"#)?;

let mut file_b = fs::File::create(root.join("ontology_b.ttl"))?;
writeln!(file_b, r#"
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://example.com/ontology_b> a owl:Ontology ;
    owl:imports <http://example.com/ontology_a> .
"#)?;

let config = Config::builder()
    .root(root.clone())
    .locations(vec![root.clone()])
    .temporary(true)
    .build()?;

let mut env = OntoEnv::init(config, false)?;
env.update()?;

let ont_b_name = NamedNode::new("http://example.com/ontology_b")?;
let ont_b_id = env.resolve(ResolveTarget::Graph(ont_b_name)).unwrap();
let closure_ids = env.get_closure(&ont_b_id, -1)?;
assert_eq!(closure_ids.len(), 2);

fs::remove_dir_all(&test_dir)?;
# Ok(())
# }
```

### Core API

| Method | Description |
|---|---|
| `OntoEnv::init(config, overwrite)` | Create (or overwrite) an environment |
| `OntoEnv::load_from_directory(root, read_only)` | Load an existing environment |
| `find_ontoenv_root()` / `find_ontoenv_root_from(path)` | Locate the nearest `.ontoenv/` by walking up |
| `env.update_all(all)` | Refresh discovered ontologies |
| `env.add(location, Overwrite, RefreshStrategy)` | Add by file/URL |
| `env.add_no_imports(location, Overwrite, RefreshStrategy)` | Add without import traversal |
| `env.add_from_bytes(location, bytes, format, Overwrite, RefreshStrategy)` | Add from in-memory RDF bytes |
| `env.get_graph(id)` | Retrieve a single ontology |
| `env.get_union_graph(ids)` | Merge multiple ontology graphs |
| `env.get_closure(id, recursion_depth)` | Compute transitive imports closure |
| `env.save_to_directory()` / `env.flush()` | Persist to `.ontoenv/store.r5tu` |

### Option Enums

| Enum | Variants | Meaning |
|---|---|---|
| `Overwrite` | `Allow`, `Preserve` | Replace existing graphs or keep original |
| `RefreshStrategy` | `Force`, `UseCache` | Bypass or reuse cached ontologies |
| `CacheMode` | `Enabled`, `Disabled` | Mirrored in Python as `use_cached_ontologies` |

`bool` values still convert via `Into` for backward compatibility.

---

## Building From Source

**Prerequisites:** Rust 1.70+, Python 3.11+ (for Python bindings)

### Rust workspace

```bash
cargo build --workspace --release   # all crates (lib/, cli/, rdf5d/)
cargo test --workspace               # all Rust tests
./test                               # Rust + Python test suites
```

### Python package (`python/`)

```bash
cd python
uv run --group dev maturin develop              # editable dev install
uv run python -m unittest discover -s tests     # run Python tests
uv run --group dev maturin build --release      # wheels → python/target/wheels/
```

### Compatibility shim (`pyontoenv-shim/`)

```bash
cd pyontoenv-shim
uv build --wheel
uv run python -m unittest discover -s tests
uv run python scripts/test_pyontoenv.py    # end-to-end install sanity check
```

This exists because I have both `ontoenv` and `pyontoenv` on PyPI — a mistake from when I restarted this project and forgot I already had the other package name.

### Version bumping

```bash
./version --check         # verify every internal crate requirement is synchronized
./version <new-version>   # set the workspace version and rebuild package lockfiles
```

Every Rust crate inherits its package version from `workspace.package.version`.
Cargo requires explicit registry versions for publishable path dependencies,
so the helper derives and validates those requirements from the same workspace
version. Do not edit the individual crate versions directly.
