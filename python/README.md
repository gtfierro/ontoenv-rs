# OntoEnv Python Bindings

## Installation

`pip install ontoenv`

## Usage

```python
from ontoenv import OntoEnv
from rdflib import Graph

# Explicitly create a persistent environment. Later processes should reopen it
# with OntoEnv.open(".") for a graph-free catalog warm start.
env = OntoEnv.create(".")

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
# env.get_graph returns a read-only store-backed rdflib.Graph
brick_graph = env.get_graph(brick_name)
print(f"Brick graph has {len(brick_graph)} triples")

# if you need a mutable in-memory graph, copy it explicitly
mutable_brick_graph = env.copy_graph(brick_name)

# get a read-only view of the full closure of the ontology, including all of its imports
# returns a tuple (ViewGraph, list[str])
brick_closure_graph, _ = env.get_closure(brick_name)
print(f"Brick closure has {len(brick_closure_graph)} triples")

# if you need a mutable materialized closure, copy it explicitly
mutable_brick_closure_graph, _ = env.copy_closure(brick_name)

# you can also add ontologies from a URL
rec_name = env.add("https://w3id.org/rec/rec.ttl")
rec_graph = env.get_graph(rec_name)
print(f"REC graph has {len(rec_graph)} triples")

# you can add an in-memory rdflib.Graph directly
in_memory = Graph()
in_memory.parse(data="""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://example.com/in-memory> a owl:Ontology .
""", format="turtle")
in_memory_name = env.add(in_memory)
print(f"Added in-memory ontology {in_memory_name}")

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

## Namespace prefixes

OntoEnv can extract namespace prefix mappings from ontology source files.
Prefixes come from both parser-level declarations (`@prefix` in Turtle,
`PREFIX` in SPARQL-style syntaxes) and SHACL `sh:declare` entries.

```python
# Get all namespaces across the entire environment
all_ns = env.get_namespaces()
# {'owl': 'http://www.w3.org/2002/07/owl#', 'brick': 'https://brickschema.org/schema/Brick#', ...}

# Get namespaces for a single ontology
ns = env.get_namespaces("https://brickschema.org/schema/1.4-rc1/Brick")

# Include namespaces from transitive owl:imports
ns_with_imports = env.get_namespaces("https://brickschema.org/schema/1.4-rc1/Brick", include_closure=True)
```

From the CLI:

```
ontoenv namespaces                                     # all namespaces
ontoenv namespaces https://example.org/my-ontology     # single ontology
ontoenv namespaces https://example.org/my-ontology --closure   # with imports
ontoenv namespaces --json                              # JSON output
```

## Custom graph store

If you want OntoEnv to write graphs into an existing Python-backed store, pass a `graph_store`
object that implements a small protocol:

```python
class GraphStore:
    # Required
    def add_graph(self, iri: str, graph: Graph, overwrite: bool = False) -> None: ...
    def get_graph(self, iri: str) -> Graph: ...   # used for read-only views (get_*)
    def remove_graph(self, iri: str) -> None: ...
    def graph_ids(self) -> list[str]: ...

    # Optional
    def copy_graph(self, iri: str) -> Graph: ...  # used for mutable copies (copy_*)
                                                  # falls back to get_graph when absent
    def size(self) -> dict[str, int]: ...         # returns {"num_graphs": ..., "num_triples": ...}
    def store_state(self) -> dict[str, str]: ...  # {"id": opaque_id, "revision": opaque_revision}
    def graph_revisions(self) -> dict[str, str]: ... # opaque revision per graph IRI
```

`copy_graph` lets stores distinguish between returning a live view (`get_graph`) and a
detached mutable copy (`copy_graph`). All `copy_*` methods (`copy_graph`, `copy_closure`,
`copy_union`, `copy_dataset`) dispatch to `copy_graph` when it is present, and fall back
to `get_graph` otherwise. The `get_*` methods always use `get_graph`.

Example:

```python
store = DictGraphStore()
env = OntoEnv(graph_store=store, temporary=True)
```

Persistent lifecycle is explicit:

```python
with OntoEnv.create("./new-env", graph_store=store) as env:
    pass

with OntoEnv.open("./existing-env", graph_store=store, read_only=True) as env:
    pass

# Import metadata from a pre-populated store. This never fetches network imports.
with OntoEnv.adopt("./adopted-env", store) as env:
    pass
```

`store_state()` enables O(1) drift detection on open. `graph_revisions()` enables
`refresh_from_store()` to reconcile only changed graphs. Without per-graph
revisions, use `graphs=[...]` or `full=True` explicitly.

Temporary environments remain catalog-free. To scan a pre-populated temporary
store without the deprecated `init_from_store` flag:

```python
env = OntoEnv(graph_store=store, temporary=True)
report = env.refresh_from_store(full=True)
print(report.added)
```

## RDFLib store with Rust SPARQL

If you want to use ontoenv as an `rdflib` store directly, use `OntoEnvStore`. This gives you
normal `rdflib.Graph` and `rdflib.Dataset` objects, but executes SPARQL through the Rust
backend instead of rdflib's Python query engine.

Use ``env.get_dataset()`` to get a read-only ``rdflib.Dataset`` view of the env.
It uses the zero-copy ``rdf5d`` snapshot when a persistent ``.ontoenv/store.r5tu``
exists and otherwise falls back to an in-memory view. Use ``env.copy_dataset()`` when
you need a mutable in-memory dataset.

```python
from rdflib import URIRef
from ontoenv import OntoEnv

env = OntoEnv(path=".demo-env", recreate=True, offline=True, search_directories=["./brick"])
brick_name = env.add("./brick/Brick.ttl")
env.update()
env.flush()

dataset = env.get_dataset()

for row in dataset.query(
    """
    SELECT ?entity ?label
    WHERE {
      GRAPH <https://brickschema.org/schema/1.4/Brick> {
        ?entity <http://www.w3.org/2000/01/rdf-schema#label> ?label .
      }
    }
    LIMIT 5
    """
):
    print(row.entity, row.label)

brick_graph = dataset.graph(URIRef(brick_name))
print(len(brick_graph))
env.close()
```

Importing `ontoenv` also registers the rdflib plugin name `"ontoenv"`, so this works too:

```python
from rdflib import Graph
import ontoenv

graph = Graph(store="ontoenv")
```

See `demo_rdflib_store.py` for a complete runnable example.

## CLI Entrypoint

Installing `ontoenv` also provides the Rust-backed `ontoenv` command-line tool:

```
pip install ontoenv
ontoenv --help
```

The CLI is identical to the standalone `ontoenv-cli` binary; see the top-level README for usage.
