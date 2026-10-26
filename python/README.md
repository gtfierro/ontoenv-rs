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

`OntoEnv.connect(path)` creates an empty managed environment if missing and
reopens the saved catalog if present. It is a class method with a required
path; `OntoEnv.connect(".")` uses the current directory. Source loading is
explicit: use `add` or `update` when files or URLs should be read.

Keep the handle for the lifetime of the program. Context managers are optional:

```python
from ontoenv import OntoEnv

env = OntoEnv.connect("./ontology-env")
try:
    site = env.add("./ontologies/site.ttl")
    graph = env.get_graph(site)
    closure, members = env.get_closure(site)

    # Capture one version when several reads must agree.
    with env.snapshot() as snapshot:
        graph = snapshot.view_graph(site)
        closure, members = snapshot.view_closure(site)
        dataset = snapshot.view_dataset(members)  # Preserves named contexts.
        editable = snapshot.copy_graph(site)

    # Keep using env in your application, and these results after the scope.
finally:
    env.close()  # Call this from the application's shutdown hook.

# Escaped views remain usable after env.close().
print(len(closure))
```

Live `env.get_graph`, `get_closure`, `get_union` and `get_dataset` calls remain
supported. Each result captures the handle's selected version. Snapshot methods
use paired `view_*` / `copy_*` names. Edits to a copy affect that independent
object. The scope releases ownership only; there is no read transaction to
commit. Managed writes publish before returning rather than at shutdown.

One writable managed handle holds the writer lock until close. Independent
read-only workers can stay open while that updater publishes or restarts:

```python
reader = OntoEnv.open("./ontology-env", read_only=True)
try:
    old = reader.snapshot()
    # After the updater process publishes a change:
    reader.refresh()  # Select saved state without fetching sources.
    current = reader.snapshot()
finally:
    reader.close()

# old and current, and their views, keep their selected versions.
```

Use `reader.refresh()` at a request boundary or on a timer. Request a new view
after refresh to read the new selection. The [service guide](../docs/how-to/use-in-a-service.rst)
includes a framework lifespan example and the updater/reader process pattern.
External stores retain their backend's concurrency and recovery guarantees.

When reopening, omitted settings retain their saved values. Explicit values,
including `False`, `"default"` and `[]`, override them:

```python
with OntoEnv.connect("./ontology-env", strict=True, offline=True) as env:
    print(env.get_ontology_names())

with OntoEnv.connect(
    "./ontology-env", strict=False, offline=False,
    resolution_policy="default", search_directories=[],
) as env:
    print(env.get_ontology_names())
```

This also covers name validation, cache/TTL settings, and include/exclude
filters. Writable opens persist overrides; read-only opens apply them only for
that session. Reconfiguration does not re-ingest sources. Discovery changes take
effect on the next `update()`. The old `create_or_use_cached` flag is removed in
0.7; use `OntoEnv.connect(path)`.

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

Prefixes you want every session to use can be saved in the environment. Saved
bindings take precedence over the prefixes ontologies declare, and are applied
to `get_namespaces()`, `get_dataset()`, `copy_dataset()`, and the views from
`get_closure()` / `get_union()`:

```python
env.bind_namespace("bldg", "urn:my-building#")   # persisted in .ontoenv/ontoenv.json
env.get_bound_namespaces()                         # {'bldg': 'urn:my-building#'}
env.unbind_namespace("bldg")
```

`dataset.bind(...)` on a Dataset from `get_dataset()` is not saved (rdflib also
calls `bind` itself for its defaults and auto-generated `ns1`-style prefixes),
and `refresh_dataset_from_env()` resets the dataset's bindings to the env's.

From the CLI:

```
ontoenv namespaces                                     # all namespaces
ontoenv namespaces https://example.org/my-ontology     # single ontology
ontoenv namespaces https://example.org/my-ontology --closure   # with imports
ontoenv namespaces --json                              # JSON output
ontoenv config bind bldg urn:my-building#              # save a prefix binding
ontoenv config unbind bldg
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
pre-populated temporary store without the removed `init_from_store` flag:

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
