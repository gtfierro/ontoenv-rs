# PyOntoenv

## Installation

`pip install pyontoenv`

## Usage

```python
from ontoenv import OntoEnv
from rdflib import Graph

# creates a new environment in the current directory, or loads
# an existing one. To use a different directory, pass the 'path'
# argument: OntoEnv(path="/path/to/env")
# OntoEnv() will discover ontologies in the current directory and
# its subdirectories
env = OntoEnv()

# add an ontology from a file path.
# env.add returns the name of the ontology, which is its URI
# e.g. "https://brickschema.org/schema/1.4-rc1/Brick"
brick_name = env.add("../brick/Brick.ttl")
print(f"Added ontology {brick_name}")

# get the graph of the ontology we just added
# env.get_graph returns an rdflib.Graph
brick_graph = env.get_graph(brick_name)
print(f"Brick graph has {len(brick_graph)} triples")

# get the full closure of the ontology, including all of its imports
# returns a tuple (rdflib.Graph, list[str])
brick_closure_graph, _ = env.get_closure(brick_name)
print(f"Brick closure has {len(brick_closure_graph)} triples")

# you can also add ontologies from a URL
rec_name = env.add("https://w3id.org/rec/rec.ttl")
rec_graph = env.get_graph(rec_name)
print(f"REC graph has {len(rec_graph)} triples")

# if you have an rdflib.Graph with an owl:Ontology declaration,
# you can transitively import its dependencies into the graph
g = Graph()
# this graph just has one triple: the ontology declaration for Brick
g.parse(data="""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<https://brickschema.org/schema/1.4-rc1/Brick> a owl:Ontology .
""")
# this will load all of the owl:imports of the Brick ontology into 'g'
env.import_dependencies(g)
print(f"Graph with imported dependencies has {len(g)} triples")
```

## Module Command (python -m)

You can initialize an environment without writing any Python by using the module command:

```
python -m ontoenv.init [options]
```

This provides a simple, Python-only frontend that mirrors the `OntoEnv(...)` constructor. It is useful when you don’t want to call into the API directly or use the Rust CLI.

Examples:

- Create (or overwrite) an env at a path:
  - `python -m ontoenv.init --path ./myproj --recreate`
- Initialize a temporary (in-memory) env for quick tasks:
  - `python -m ontoenv.init --temporary --root .`
- Open an existing env in read-only mode:
  - `python -m ontoenv.init --path ./myproj --read-only`
- Discover ontologies under a search directory when initializing:
  - `python -m ontoenv.init --path ./myproj --recreate --search-dir ./brick`

Arguments (mirror `OntoEnv` kwargs):

- `--path PATH`: Root directory where `.ontoenv` lives or will be created
- `--recreate`: Create or overwrite an env at `--path`
- `--read-only`: Open the env in read-only mode
- `--search-dir DIR`: Directory to search for ontologies (repeatable)
- `--require-ontology-names`: Require explicit ontology names
- `--strict`: Enable strict mode (treat resolution failures as errors)
- `--offline`: Disable network access for resolution
- `--resolution-policy NAME`: Resolution policy to use (default: `default`)
- `--root DIR`: Discovery start directory when not recreating (default: `.`)
- `--include PATTERN`: Include pattern for discovery (repeatable)
- `--exclude PATTERN`: Exclude pattern for discovery (repeatable)
- `--temporary`: Use an in-memory environment (no files created)
- `--no-search`: Disable local directory search

Behavior:

- Prints the environment store path on success when persisted (non-temporary).
- Exits with code 0 on success; on error prints a message and exits non‑zero.
