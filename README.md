# OntoEnv

`ontoenv` is an environment manager for ontology management. It eventually wants to be a package manager for RDF ontologies and graphs.

- A CLI tool (`cargo install ontoenv-cli`)
- `ontoenv`, a [Rust library](https://docs.rs/ontoenv/latest/ontoenv/)
- `pyontoenv`, a [Python library](https://pypi.org/project/pyontoenv/)

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

When initialized, `ontoenv` searches for all local files defining ontologies, identifies their dependencies, and then recursively pulls in those dependencies, *their* dependencies, and so on.
It saves this in a local [Oxigraph](https://github.com/oxigraph/oxigraph) database inside the local `.ontoenv`.

## Command Line Interface

### Installation

- If you have Rust installed, you can install the tool with `cargo install ontoenv-cli`
- Download a binary from the [Releases](https://github.com/gtfierro/ontoenv-rs/releases) tab

### Usage

#### Initialization

Begin by initializing an `ontoenv` workspace in a directory containing some ontology files (Turtle files, etc).

```ignore
ontoenv init
```

This may take a couple minutes. `ontoenv` searches for all local files defining ontologies, identifies their dependencies, and then recursively pulls in those dependencies, *their* dependencies, and so on. It is possible to adjust which directories `ontoenv` searches for, which files it traverses, and whether it pulls ontologies from the web.

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

`ontoenv` stores its configuration and internal database in a `.ontoenv` directory placed in directory from where you ran `ontoenv init`.

#### Refreshing

Refresh the workspace to account for changes to local files. `ontoenv` will use the timestamps on the local files to determine which files to load. This means that refreshing the workspace is often much faster than a full initialization.

Refreshing the graph uses the same parameters as given during `ontoenv init`.
To change these parameters, just run `ontoenv init` again with the desired flags and parameters.

#### Importing Dependencies

`ontoenv` can import all dependencies (immediate and transitive) into a unified graph.
This is often helpful for passing to reasoners or query processors; while many of these can deal with importing multiple graphs, it is much more convenient to have a single file one can ship around.
We refer to the resulting "unified graph" as the *imports closure*.

`ontoenv closure <root ontology name>` computes the imports closure and places it into an `output.ttl` file (or a location of your choice).
There are a several flags one can provide for this process

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

#### Listing Ontologies

`ontoenv list-ontologies` will display a list of ontology names in the workspace.

`ontoenv dump` will print out an alphabetized list of all ontologies in the workspace, their imports, number of triples, and other metadata.

If GraphViz is installed, `ontoenv dep-graph` will output a PDF graph representation of the imports closure.

## Python Library

##### Installation

`pip install pyontoenv`

#### Usage

Here is a basic example of how to use the `pyontoenv` Python library. This example will:
1. Create a temporary directory.
2. Write two simple ontologies to files in that directory, where one imports the other.
3. Configure and initialize `ontoenv` to use this directory.
4. Compute the dependency closure of one ontology to demonstrate that `ontoenv` correctly resolves and includes the imported ontology.

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
