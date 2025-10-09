# OntoEnv

`ontoenv` is a lightweight environment manager for RDF ontologies and their imports. It helps you:

- Discover ontologies locally and on the web
- Resolve and materialize `owl:imports` closures
- Query and export graphs

Project components:
- CLI: `ontoenv` (installable via `cargo install ontoenv-cli`)
- Rust library: [`ontoenv`](https://docs.rs/ontoenv/latest/ontoenv/)
- Python bindings: [`pyontoenv`](https://pypi.org/project/pyontoenv/)

## Overview

Imagine you have an RDF graph which imports some ontologies in order to use those concepts.
Those ontologies might in import other ontologies, and so on.

The design goals of this project are:
- **be lightweight**:  big fancy ontology tools will handle ontology imports automatically, but do so within a heavyweight GUI and usually without an easy-to-use API; I wanted something that could be used in a Python library or a command line tool
- **configurable**: when doing ontology development, I want to refer to some files locally, and others on the web; I want to be able to control which files are included and which are not.
- **fast**: I want to be able to quickly refresh my workspace when I make changes to local files.

## How does it work?

Specifically, `ontoenv` looks for patterns like the following inside local ontology files:

```ttl
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <urn:my_ontology/> .

<urn:my_ontology> rdf:type owl:Ontology ;
    owl:imports <https://brickschema.org/schema/1.4/Brick>,
                <http://qudt.org/2.1/vocab/quantitykind> .
```

When initialized, `ontoenv` searches the specified directories for ontology declarations, identifies their `owl:imports`, and recursively pulls in dependencies. Runtime queries operate on an in‑memory Oxigraph store. Persistent on‑disk state uses a compact RDF5D file at `.ontoenv/store.r5tu` with single‑writer, shared reader locking.

### Canonical IRIs and Source URLs

Ontologies fetched from a URL often declare a different, usually versioned, ontology IRI inside the file. `ontoenv` now remembers that relationship. When an ontology is added we record the source location and, if its declared name differs, create an alias from the normalized URL to the canonical ontology identifier. Future `owl:imports` that reference the versionless URL will therefore reuse the already downloaded ontology instead of refetching it. Removing an ontology clears any aliases associated with it, and loading an existing environment rebuilds the mapping automatically.

## CLI

### Installation

- Install from crates.io with `cargo install --locked ontoenv-cli`
- From a local checkout, run `cargo install --path cli --locked` to build the current workspace
- Download a binary from the [Releases](https://github.com/gtfierro/ontoenv-rs/releases) tab

### Usage

#### init

Begin by initializing an `ontoenv` workspace in a directory containing some ontology files (Turtle files, etc).

```ignore
ontoenv init
```

Initializes `.ontoenv/` in the current directory (or specified root), discovers ontologies, and loads dependencies. You must run `init` once per environment. Subsequent commands will auto‑discover the nearest `.ontoenv/` in parent directories.

```ignore
$ ontoenv init -h
Create a new ontology environment

Usage: ontoenv init [OPTIONS] [SEARCH_DIRECTORIES]...

Arguments:
  [SEARCH_DIRECTORIES]...  Directories to search for ontologies. If not provided, the current directory is used

Options:
      --overwrite                 Overwrite the environment if it already exists
  -r, --require-ontology-names  Require ontology names to be unique; will raise an error if multiple ontologies have the same name
  -s, --strict                  Strict mode - will raise an error if an ontology is not found
  -o, --offline                 Offline mode - will not attempt to fetch ontologies from the web
  -p, --policy <POLICY>         Resolution policy for determining which ontology to use when there are multiple with the same name. One of 'default', 'latest', 'version' [default: default]
  -n, --no-search               Do not search for ontologies in the search directories
  -i, --includes <INCLUDES>...  Glob patterns for which files to include, defaults to ['*.ttl','*.xml','*.n3']
  -e, --excludes <EXCLUDES>...  Glob patterns for which files to exclude, defaults to []
  -h, --help                    Print help
```

Offline mode in particular is helpful when you want to limit which ontologies get loaded. Simply download the ontologies you want, and then enable offline mode.

Examples:
- `ontoenv init` — initialize in current directory
- `ontoenv init ./ontologies ./models` — initialize and search these directories
- `ontoenv init --overwrite --offline ./ontologies` — rebuild from scratch and work offline

#### Local State

- Directory: `.ontoenv/`
- Persistent store: `.ontoenv/store.r5tu` (RDF5D)
- Lock file: `.ontoenv/store.lock` (single writer, shared readers)

### Behavior

- Discovery: Commands (except `init`) discover an environment by walking up parent directories from the current working directory, looking for `.ontoenv/`.
- Override: Set `ONTOENV_DIR` to point to a specific environment; if it points at a `.ontoenv` directory the parent of that directory is used as the root.
- Creation: Only `ontoenv init` creates an environment on disk. Other commands will error if no environment is found.
- Positional search directories: Only `ontoenv init` accepts positional search directories (LOCATIONS). Other commands ignore trailing positionals.
- Temporary mode: Pass `--temporary` to run with an in‑memory environment (no `.ontoenv/`).

#### update

- Refreshes the environment based on file timestamps and configuration.
- Re‑run `init` to change search paths or flags.

Examples:
- `ontoenv update` — refresh only changed/added files
- `ontoenv update --all` — rebuild the in‑memory view from sources

#### closure

Compute and optionally write the imports closure (union of a graph and its transitive imports). Useful for reasoning, exchange, or exporting a single file.

Examples:
- `ontoenv closure http://example.org/ont/MyOntology` (writes `output.ttl`)
- `ontoenv closure http://example.org/ont/MyOntology result.ttl` (auto‑rewrites SHACL prefixes and removes owl:imports)
- To disable either behavior:
  - `ontoenv closure http://example.org/ont/MyOntology --no-rewrite-sh-prefixes`
  - `ontoenv closure http://example.org/ont/MyOntology --keep-owl-imports`

#### get

Retrieve a single ontology graph from the environment and write it to STDOUT or a file in a chosen serialization format.

Examples:
- `ontoenv get http://example.org/ont/MyOntology` — prints Turtle to STDOUT
- `ontoenv get http://example.org/ont/MyOntology --format jsonld` — prints JSON‑LD to STDOUT
- `ontoenv get http://example.org/ont/MyOntology --output my.ttl` — writes Turtle to `my.ttl`
- Disambiguate when multiple copies share the same IRI (different locations):
  - `ontoenv get http://example.org/ont/MyOntology --location ./ontologies/MyOntology-1.4.ttl`
  - `ontoenv get http://example.org/ont/MyOntology -l https://example.org/MyOntology-1.3.ttl`

Notes:
- Supported formats: `turtle` (default), `ntriples`, `rdfxml`, `jsonld`.
- `--output` writes to a file; omit to print to STDOUT.
- `--location` accepts a file path or URL and is only needed to disambiguate when multiple sources exist for the same IRI.

#### Other commands

- `ontoenv dump` — show ontologies, imports, sizes, and metadata
- `ontoenv dep-graph` — export a GraphViz import dependency graph (PDF) if GraphViz is available
- `ontoenv status` — human-friendly status; add `--json` for machine‑readable
- `ontoenv update` — refresh discovered ontologies
- `ontoenv list ontologies` — ontology names in the environment; add `--json` for JSON array
- `ontoenv list missing` — missing imports (i.e. not found in environment); add `--json` for JSON array
- `ontoenv why <IRI> [<IRI> ...]` — show who imports the given ontology as paths; add `--json` to emit a single JSON document mapping each IRI to path arrays

## Python API (`pyontoenv`)

##### Installation

`pip install pyontoenv` (requires Python 3.9+; prebuilt wheels ship for common platforms. Building from source needs a Rust toolchain.)

### Basic usage

Example: create a temporary environment, discover ontologies, and compute a closure.

```python
import tempfile
from pathlib import Path
from ontoenv import OntoEnv
from rdflib import Graph

# create a temporary directory to store our ontology files
with tempfile.TemporaryDirectory() as temp_dir:
    root = Path(temp_dir)
    # create a dummy ontology file for ontology A
    ontology_a_content = """
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <http://example.com/ontology_a#> .
<http://example.com/ontology_a> a owl:Ontology .
"""
    (root / "ontology_a.ttl").write_text(ontology_a_content)

    # create a dummy ontology file for ontology B which imports A
    ontology_b_content = """
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <http://example.com/ontology_b#> .
<http://example.com/ontology_b> a owl:Ontology ;
    owl:imports <http://example.com/ontology_a> .
"""
    (root / "ontology_b.ttl").write_text(ontology_b_content)

    # make the environment. We use temporary=True so it doesn't create a .ontoenv dir
    env = OntoEnv(search_directories=[str(root)], strict=False, offline=True, temporary=True)

    # list the ontologies found
    print("Ontologies found:", env.get_ontology_names())

    # compute closure for ontology B and insert it into a graph
    g = Graph()
    env.get_closure("http://example.com/ontology_b", destination_graph=g)
    # The closure should contain triples from both A and B.
    # Each ontology has 1 triple, so the union should have 2.
    print(f"Closure of ontology_b has {len(g)} triples")
    assert len(g) == 2

    # get just the graph for ontology A
    g_a = env.get_graph("http://example.com/ontology_a")
    print(f"Graph of ontology_a has {len(g_a)} triples")
    assert len(g_a) == 1
```

### Key methods

- Constructor: `OntoEnv(path=None, recreate=False, read_only=False, search_directories=None, require_ontology_names=False, strict=False, offline=False, resolution_policy="default", root=".", includes=None, excludes=None, temporary=False, no_search=False)`
  - `offline`: don’t fetch remote ontologies
  - `temporary`: in‑memory only (no `.ontoenv/`)
- `update(all=False)`: refresh discovered ontologies
- `add(location, fetch_imports=True) -> str`: add graph from file or URL; returns graph IRI
- `get_graph(name) -> rdflib.Graph`: get just one ontology graph
- `get_closure(name, destination_graph=None, rewrite_sh_prefixes=True, remove_owl_imports=True, recursion_depth=-1) -> (Graph, list[str])`
- `import_dependencies(graph, fetch_missing=False) -> list[str]`: load imports into an rdflib graph, remove its `owl:imports`, and return the sorted IRIs that were imported
- `list_closure(name, recursion_depth=-1) -> list[str]`: list IRIs in the closure
- `get_importers(name) -> list[str]`: ontologies that import `name`
- `to_rdflib_dataset() -> rdflib.Dataset`: in‑memory Dataset with one named graph per ontology
- `store_path() -> Optional[str]`: path to `.ontoenv/` (persistent envs) or `None` (temporary)
- `close()`: persist (if applicable) and release resources

### Module command

- `python -m ontoenv.init --help` exposes a Python-only CLI that mirrors the `OntoEnv(...)` constructor flags.
- The launcher always passes `recreate=True`, so pointing it at a persistent path will rebuild the environment before exiting.
- Successful runs print the resolved store path; combine with `--temporary` for in-memory experiments that avoid touching disk.

### Behavior

- Strict Git‑like:
  - Temporary environment: `OntoEnv(temporary=True)` creates an in‑memory environment (no `.ontoenv/`).
  - Create/overwrite on disk: `OntoEnv(path=..., recreate=True)` explicitly creates a new environment at `path` (or overwrites if it exists).
  - Discover and load: Otherwise, the constructor walks up from `path` (or `root=.` if `path` is None) to find an existing `.ontoenv/`. If found, it loads it; if not, it raises `ValueError` with a hint to use `recreate=True` or `temporary=True`.
  - Flags such as `offline`, `strict`, `search_directories`, `includes`, `excludes` apply to created environments; loading respects the saved configuration.

#### get_closure vs import_dependencies

- `get_closure(name, ...)` computes the transitive imports closure for the ontology identified by `name` and builds the union of all graphs in that closure.
  - Returns: a new rdflib Graph (or writes into `destination_graph` if provided) plus the ordered list of ontology IRIs in the closure.
  - Options: can rewrite SHACL prefix blocks to the chosen base ontology and remove `owl:imports` statements in the merged result.
  - Use when you need a single, self‑contained graph representing an ontology and all of its imports (for reasoning, exchange, or export).

- `import_dependencies(graph, fetch_missing=False)` scans an existing rdflib Graph for ontology declarations and `owl:imports`, then augments that same Graph by loading the referenced ontologies (from the environment, or from the web if `fetch_missing=True`).
  - Returns: the list of ontology IRIs that were imported.
  - Mutates: the provided rdflib Graph in‑place (does not create a union graph per se; it enriches the given graph with imported triples).
  - Use when you already have a Graph and want to populate it with the triples from its declared imports, respecting your environment’s offline/strict settings.

## Rust Library

[Docs](https://docs.rs/crate/ontoenv)

### Usage

Here is a basic example of how to use the `ontoenv` Rust library. This example will:
1. Create a temporary directory.
2. Write two simple ontologies to files in that directory, where one imports the other.
3. Configure and initialize `ontoenv` to use this directory.
4. Compute the dependency closure of one ontology to demonstrate that `ontoenv` correctly resolves and includes the imported ontology.

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
// Set up a temporary directory for the example
let test_dir = PathBuf::from("target/doc_test_temp_readme");
if test_dir.exists() {
    fs::remove_dir_all(&test_dir)?;
}
fs::create_dir_all(&test_dir)?;
let root = test_dir.canonicalize()?;

// Create a dummy ontology file for ontology A
let ontology_a_path = root.join("ontology_a.ttl");
let mut file_a = fs::File::create(&ontology_a_path)?;
writeln!(file_a, r#"
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <http://example.com/ontology_a#> .
<http://example.com/ontology_a> a owl:Ontology .
"#)?;

// Create a dummy ontology file for ontology B which imports A
let ontology_b_path = root.join("ontology_b.ttl");
let mut file_b = fs::File::create(&ontology_b_path)?;
writeln!(file_b, r#"
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <http://example.com/ontology_b#> .
<http://example.com/ontology_b> a owl:Ontology ;
    owl:imports <http://example.com/ontology_a> .
"#)?;

// Configure ontoenv
let config = Config::builder()
    .root(root.clone())
    .locations(vec![root.clone()])
    .temporary(true) // Use a temporary environment
    .build()?;

// Initialize the environment
let mut env = OntoEnv::init(config, false)?;
env.update()?;

// Check that our ontologies were loaded
let ontologies = env.ontologies();
assert_eq!(ontologies.len(), 2);

// Get the dependency closure for ontology B
let ont_b_name = NamedNode::new("http://example.com/ontology_b")?;
let ont_b_id = env.resolve(ResolveTarget::Graph(ont_b_name)).unwrap();
let closure_ids = env.get_closure(&ont_b_id, -1)?;

// The closure should contain both ontology A and B
assert_eq!(closure_ids.len(), 2);
let closure_names: HashSet<String> = closure_ids.iter().map(|id| id.to_uri_string()).collect();
println!("Closure contains: {:?}", closure_names);
assert!(closure_names.contains("http://example.com/ontology_a"));
assert!(closure_names.contains("http://example.com/ontology_b"));


// Clean up
fs::remove_dir_all(&test_dir)?;
# Ok(())
# }
```

### Core Rust API (selected)

- `OntoEnv::init(config, overwrite) -> OntoEnv`
- `OntoEnv::load_from_directory(root, read_only) -> OntoEnv`
- `OntoEnv::update_all(all: bool)`
- `OntoEnv::add(location, Overwrite, RefreshStrategy) -> GraphIdentifier`
- `OntoEnv::add_no_imports(location, Overwrite, RefreshStrategy) -> GraphIdentifier`
- `OntoEnv::get_graph(id) -> Graph`
- `OntoEnv::get_union_graph(ids)` and `get_closure(id, recursion_depth)`
- `OntoEnv::save_to_directory()`, `flush()` (persists to `.ontoenv/store.r5tu`)

Persistent storage details
- On-disk: RDF5D file `.ontoenv/store.r5tu` (single writer, shared readers, atomic writes)
- Runtime: in-memory Oxigraph store for fast queries

### Behavior

- Discovery helpers:
  - `find_ontoenv_root()` and `find_ontoenv_root_from(path)`: walk up parent directories to locate the root that contains `.ontoenv/`.
  - Load: `OntoEnv::load_from_directory(root, read_only)` loads an existing environment.
- Creation:
  - `OntoEnv::init(config, overwrite)` explicitly creates (or overwrites) an environment on disk.
  - `OntoEnv::add(..., Overwrite::Allow, RefreshStrategy::UseCache)` is the common way to add an ontology, while `RefreshStrategy::Force` skips cache reuse.
- Recommended pattern:
  - Try discovery (`find_ontoenv_root()`), then `load_from_directory`; if not found, prompt/init explicitly.
  - Use `config.temporary = true` (via `Config::builder`) and `OntoEnv::init` for in‑memory use cases.

### Option enums

The Rust API now exposes expressive enums instead of opaque booleans:

- `Overwrite::{Allow, Preserve}` — replace existing graphs or keep the original.
- `RefreshStrategy::{Force, UseCache}` — bypass or reuse cached ontologies.
- `CacheMode::{Enabled, Disabled}` — persisted in `Config` and mirrored in Python as the `use_cached_ontologies` boolean.

From older code that passed `true`/`false`, use `Overwrite::Allow`/`Preserve` and `RefreshStrategy::Force`/`UseCache`. `bool` values still convert via `Into`, so existing code can migrate incrementally.
