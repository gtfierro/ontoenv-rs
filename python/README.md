# OntoEnv Python Bindings

## Installation

`pip install ontoenv`

## Usage

```python
from ontoenv import OntoEnv, UnresolvedImportError
from rdflib import Graph

# Connect creates on first use and graph-free warm-opens on later runs.
env = OntoEnv.connect(".")

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

# if you have an rdflib.Graph with an owl:imports declaration,
# you can transitively import its dependencies into the graph
g = Graph()
g.parse(data="""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://example.com/application> a owl:Ontology ;
    owl:imports <https://brickschema.org/schema/1.4/Brick> .
""", format="turtle")
# Fetch imports not already cached and merge the available closure into g.
# In non-strict mode unavailable imports are skipped without leaving a
# catalog.pending recovery marker.
env.import_dependencies(g, fetch_missing=True)
print(f"Graph with imported dependencies has {len(g)} triples")

# Known unresolved targets have a dedicated exception type.
for missing in env.missing_imports():
    try:
        env.copy_graph(missing)
    except UnresolvedImportError:
        print(f"Import is unavailable: {missing}")
env.close()
```

## Using a persistent environment

For persistent use, OntoEnv saves your settings plus a small index of ontology
names, imports, aliases, locations, namespaces, and hashes. This lets later
connections answer dependency questions without rereading every RDF triple.

Use `connect` for normal application code:

```python
env = OntoEnv.connect("./ontology-env")
site = env.add("./ontologies/site.ttl")
```

On the first run, this creates the environment directory, saves its settings,
and initializes graph storage. On later runs, the same call loads the saved
ontology index. There is no need to check whether the environment exists
before connecting.

Saved settings are retained when their options are omitted. To deliberately
change a setting, pass an explicit value:

```python
env = OntoEnv.connect(
    "./ontology-env",
    strict=True,
    offline=True,
    resolution_policy="latest",
)
env.close()
env = OntoEnv.connect(
    "./ontology-env",
    strict=False,
    offline=False,
    resolution_policy="default",
    search_directories=[],  # an empty list explicitly clears saved paths
)
```

This rule also covers ``require_ontology_names``, ``use_cached_ontologies``,
``remote_cache_ttl_secs``, and all include/exclude settings. A read-only
connection applies explicit overrides only for that session. Reconfiguration
does not re-ingest the graph store; changed discovery paths and filters are
used by the next explicit ``update()``.

The context manager is optional. For a long-lived service, connect once during
startup and keep the object in application state:

```python
env = OntoEnv.connect("/srv/ontology-env")
application_state.ontoenv = env

# Reuse application_state.ontoenv in request handlers, then close it from the
# web framework's shutdown hook.
application_state.ontoenv.close()
```

For a short script, `with` performs the same cleanup automatically:

```python
with OntoEnv.connect("/srv/ontology-env") as env:
    print(env.get_ontology_names())
```

For a multi-process server, each read-only worker may open its own
`OntoEnv.connect(path, read_only=True)`. A persistent environment permits one
writer, so do not create an independent writable environment in every worker;
route mutations through one writer and serialize them at the application level.

For custom stores, ``sync="auto"`` reconnects quickly and reads only graphs the
store can identify as changed. If the store cannot identify those graphs,
OntoEnv asks for an explicit ``sync="full"`` instead of silently scanning
everything. This synchronizes direct store changes; it does not check ontology
files or URLs. Call `env.update()` after connecting when source files, remote
sources, and their imports should be refreshed.

Most applications never need a different lifecycle method. `create` is useful
for a setup command that must fail if the environment already exists. `open`
is useful when deployment must have prepared the environment in advance and
startup must neither create nor synchronize it. `adopt` explicitly reads every
graph in a populated custom store once and records the ontology information
OntoEnv needs; it does not fetch network imports. For tests and notebooks that
should write no environment files, use `OntoEnv(temporary=True)`.

Refreshing source files and URLs is separate from reconciling a custom graph
store. `env.update()` checks changed local sources and expired remote sources,
then follows their imports. `env.update(force=True)` forces every known source
and its dependencies to be reread. To update one ontology and its imports,
pass its file or URL directly; its stored graph is replaced automatically:

```python
env.update(source)
env.update(source, force=True)  # reread even when the cached copy looks current
```

When graphs were changed directly in a custom store, use
`refresh_from_store()` instead. A targeted refresh accepts exact graph IDs. To
include the dependency closure currently known to OntoEnv:

```python
report = env.refresh_from_store(graphs=env.list_closure(root))
```

If the external edit added an entirely new imported graph, use incremental
refresh with a store that reports per-graph changes, or request
`refresh_from_store(full=True)`.

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

For a persistent custom store, use ``connect``:

```python
env = OntoEnv.connect("./environment", graph_store=store)
```

If the store implements the optional change-reporting methods, OntoEnv can
notice external edits and read only the affected graphs. Otherwise, writes
through ``env`` remain synchronized automatically, while out-of-band edits
require ``sync="full"``.

Temporary environments save no persistent ontology index. To scan a
pre-populated temporary store without the deprecated `init_from_store` flag:

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
