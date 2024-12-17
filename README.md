# OntoEnv

`ontoenv` is an environment manager for ontology management. It eventually wants to be a package manager for RDF ontologies and graphs.

## Overview

Imagine you have an RDF graph which imports 

## Command Line Interface

### Installation

- If you have Rust installed, you can install the tool with `cargo install ontoenv-cli`
- Download a binary from the [Releases](https://github.com/gtfierro/ontoenv-rs/releases) tab

### Usage

#### Initialization

Begin by initializing an `ontoenv` workspace in a directory containing some ontology files (Turtle files, etc).

```
ontoenv init
```

This may take a couple minutes. `ontoenv` searches for all local files defining ontologies, identifies their dependencies, and then recursively pulls in those dependencies, *their* dependencies, and so on.

Specifically, `ontoenv` looks for patterns like the following inside local ontology files:

```ttl
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <urn:my_ontology/> .

<urn:my_ontology> rdf:type owl:Ontology ;
    owl:imports <https://brickschema.org/schema/1.4/Brick>,
                <http://qudt.org/2.1/vocab/quantitykind> .
```

It is possible to adjust which directories `ontoenv` searches for, which files it traverses, and whether it pulls ontologies from the web.

```
$ ontoenv init -h
Create a new ontology environment

Usage: ontoenv init [OPTIONS] [SEARCH_DIRECTORIES]...

Arguments:
  [SEARCH_DIRECTORIES]...  Directories to search for ontologies. If not provided, the current directory is used

Options:
  -r, --require-ontology-names  Require ontology names to be unique; will raise an error if multiple ontologies have the same name
  -s, --strict                  Strict mode - will raise an error if an ontology is not found
  -o, --offline                 Offline mode - will not attempt to fetch ontologies from the web
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

`ontoenv get-closure <root ontology name>` computes the imports closure and places it into an `output.ttl` file (or a location of your choice).
There are a several flags one can provide for this process

```
$ Compute the owl:imports closure of an ontology and write it to a file

Usage: ontoenv get-closure [OPTIONS] <ONTOLOGY> [DESTINATION]

Arguments:
  <ONTOLOGY>     The name (URI) of the ontology to compute the closure for
  [DESTINATION]  The file to write the closure to, defaults to 'output.ttl'

Options:
  -r, --rewrite-sh-prefixes <REWRITE_SH_PREFIXES>
          Rewrite the sh:prefixes declarations to point to the chosen ontology, defaults to true [default: true] [possible values: true, false]
  -r, --remove-owl-imports <REMOVE_OWL_IMPORTS>
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

```python
from ontoenv import Config, OntoEnv
from rdflib import Graph

# create config object. This assumes you have a 'brick' folder locally storing some ontologies
cfg = Config(["brick"], strict=False, offline=True)
# can also create an 'empty' config object if there are no local ontologies
# cfg = Config(strict=False, offline=True)

# make the environment
env = OntoEnv(cfg)

# compute closure for a given ontology and insert it into a graph
g = Graph()
env.get_closure("https://brickschema.org/schema/1.4/Brick", g)

# import all dependencies from a graph
brick = Graph()
brick.parse("brick/Brick.ttl", format="turtle")
env.import_dependencies(brick)

# get a graph by IRI
rec = env.get_graph("https://w3id.org/rec")

# add an ontology to a graph by IRI
env.import_graph(brick, "https://w3id.org/rec")

# get an rdflib.Dataset (https://rdflib.readthedocs.io/en/stable/apidocs/rdflib.html#rdflib.Dataset)
ds = env.to_rdflib_dataset()
for graphname in ds.graphs():
    graph = ds.graph(graphname)
    print(f"Graph {graphname} has {len(graph)} triples")
```

## Rust Library

[Docs](https://docs.rs/crate/ontoenv)
