# PyOntoenv

## Installation

`pip install pyontoenv`

## Usage

```python
from ontoenv import Config, OntoEnv
from rdflib import Graph

cfg = Config(["../brick"], strict=False, offline=True)

# make environment
env = OntoEnv(cfg)

g = Graph()
# put the transitive owl:imports closure into 'g'
env.get_closure("https://brickschema.org/schema/1.4-rc1/Brick", g)

# or, get the graph directly
g = env.get_closure("https://brickschema.org/schema/1.4-rc1/Brick")

brick = Graph()
brick.parse("Brick.ttl", format="turtle")
# transitively import dependencies into the 'brick' graph, using the owl:imports declarations
env.import_dependencies(brick)

# pull Brick graph out of environment
brick = env.get_graph("https://brickschema.org/schema/1.4-rc1/Brick")

# import graphs by name
env.import_graph(brick, "https://w3id.org/rec")
```
