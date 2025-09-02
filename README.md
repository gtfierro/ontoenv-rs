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

## CLI

### Installation

- If you have Rust installed, you can install the tool with `cargo install ontoenv-cli`
- Download a binary from the [Releases](https://github.com/gtfierro/ontoenv-rs/releases) tab

### Usage

#### init

Begin by initializing an `ontoenv` workspace in a directory containing some ontology files (Turtle files, etc).

```ignore
ontoenv init
```

Initializes `.ontoenv/`, discovers ontologies, and loads dependencies. Tune search paths and behavior with flags.

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

#### Local State

- Directory: `.ontoenv/`
- Persistent store: `.ontoenv/store.r5tu` (RDF5D)
- Lock file: `.ontoenv/store.lock` (single writer, shared readers)

#### update

- Refreshes the environment based on file timestamps and configuration.
- Re‑run `init` to change search paths or flags.

Examples:
- `ontoenv update` — refresh only changed/added files
- `ontoenv init --overwrite` — rebuild from scratch

#### closure

Compute and optionally write the imports closure (union of a graph and its transitive imports). Useful for reasoning, exchange, or exporting a single file.

```ignore
$ Compute the owl:imports closure of an ontology and write it to a file

Usage: ontoenv closure [OPTIONS] <ONTOLOGY> [DESTINATION]

Arguments:
  <ONTOLOGY>     The name (URI) of the ontology to compute the closure for
  [DESTINATION]  The file to write the closure to, defaults to 'output.ttl'

Options:
      --rewrite-sh-prefixes <REWRITE_SH_PREFIXES>
          Rewrite the sh:prefixes declarations to point to the chosen ontology, defaults to true [default: true] [possible values: true, false]
      --remove-owl-imports <REMOVE_OWL_IMPORTS>
          Remove owl:imports statements from the closure, defaults to true [default: true] [possible values: true, false]
  -h, --help
          Print help
```

#### Other commands

- `ontoenv list-ontologies` — list ontology IRIs known to the environment
- `ontoenv dump` — show ontologies, imports, sizes, and metadata
- `ontoenv dep-graph` — export a GraphViz import dependency graph (PDF) if GraphViz is available

## Python API (`pyontoenv`)

##### Installation

`pip install pyontoenv`

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
- `import_dependencies(graph, fetch_missing=False) -> list[str]`: load imports into an rdflib graph
- `list_closure(name, recursion_depth=-1) -> list[str]`: list IRIs in the closure
- `get_importers(name) -> list[str]`: ontologies that import `name`
- `to_rdflib_dataset() -> rdflib.Dataset`: in‑memory Dataset with one named graph per ontology
- `store_path() -> Optional[str]`: path to `.ontoenv/` (persistent envs) or `None` (temporary)
- `close()`: persist (if applicable) and release resources

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
let closure = env.get_dependency_closure(&ont_b_id)?;

// The closure should contain both ontology A and B
assert_eq!(closure.len(), 2);
let closure_names: HashSet<String> = closure.iter().map(|id| id.to_uri_string()).collect();
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
- `OntoEnv::add(location, overwrite) -> GraphIdentifier`
- `OntoEnv::get_graph(id) -> Graph`
- `OntoEnv::get_union_graph(ids)` and `get_closure(id, recursion_depth)`
- `OntoEnv::save_to_directory()`, `flush()` (persists to `.ontoenv/store.r5tu`)

Persistent storage details
- On-disk: RDF5D file `.ontoenv/store.r5tu` (single writer, shared readers, atomic writes)
- Runtime: in-memory Oxigraph store for fast queries
