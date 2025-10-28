# OntoEnv Python Bindings

## Installation

`pip install ontoenv`

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

# When you add from a URL whose declared ontology name differs (for example a
# versioned IRI served at a versionless URL), ontoenv records that alias. You
# can later refer to the ontology by either the canonical name or the original
# URL when resolving imports or querying.

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

## CLI Entrypoint

Installing `ontoenv` also provides the Rust-backed `ontoenv` command-line tool:

```
pip install ontoenv
ontoenv --help
```

The CLI is identical to the standalone `ontoenv-cli` binary; see the top-level README for usage.
