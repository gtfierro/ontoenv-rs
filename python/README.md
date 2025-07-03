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
# env.get returns an rdflib.Graph
brick_graph = env.get(brick_name)
print(f"Brick graph has {len(brick_graph)} triples")

# get the full closure of the ontology, including all of its imports
# also returns an rdflib.Graph
brick_closure_graph = env.get_closure(brick_name)
print(f"Brick closure has {len(brick_closure_graph)} triples")

# you can also add ontologies from a URL
rec_name = env.add("https://w3id.org/rec/rec.ttl")
rec_graph = env.get(rec_name)
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
