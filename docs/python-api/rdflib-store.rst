RDFLib Store
============

.. raw:: html

   <div class="oe-section-intro">
     OntoEnv ships a native <code>rdflib</code> store implementation. Use it when you want
     normal <code>rdflib.Graph</code> / <code>rdflib.Dataset</code> objects, but want SPARQL
     execution to stay in Rust via <code>spargebra</code> and <code>spareval</code>.
   </div>

When to use it
--------------

Use ``OntoEnvStore`` when you want:

- ``rdflib.Graph.query(...)`` and ``rdflib.Dataset.query(...)`` to execute through the Rust
  backend instead of rdflib's Python query engine.
- Normal rdflib graph APIs such as ``add``, ``remove``, ``triples``, ``contexts``, and
  namespace bindings.
- A lightweight in-memory dataset for scripting, testing, or embedding.

The store is also registered as the rdflib plugin name ``"ontoenv"`` once the
``ontoenv`` package is imported.

Do not confuse this with ``OntoEnv(graph_store=...)``:

- ``OntoEnvStore`` is an ``rdflib.store.Store`` implementation.
- ``graph_store=`` is a separate OntoEnv-specific protocol for plugging your own graph
  persistence into ontology ingestion.

Environment-backed usage
------------------------

The usual workflow is to build an ``OntoEnv`` environment first, then materialize it into
an ``OntoEnvStore``-backed dataset for SPARQL and graph access.

.. code-block:: python

   from rdflib import URIRef
   from ontoenv import OntoEnv

   env = OntoEnv(
       path=".demo-env",
       recreate=True,
       offline=True,
       search_directories=["./brick"],
   )
   brick_name = env.add("./brick/Brick.ttl")
   env.update()
   env.flush()

   dataset = env.as_dataset()

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

If you prefer rdflib's plugin lookup:

.. code-block:: python

   from rdflib import Graph
   import ontoenv  # registers the "ontoenv" store plugin

   graph = Graph(store="ontoenv")

``env.as_dataset()`` returns a read-only ``rdflib.Dataset`` view of the env.
It binds the namespaces known to the environment and keys each named graph by its
ontology IRI. The Dataset reflects the env's state at the time of the call; call it
again (or ``env.refresh_dataset(dataset)``) after ``env.flush()`` to pick
up changes.

The ``backend`` parameter selects the storage strategy:

- ``"auto"`` (default) — use ``"rdf5d"`` if ``.ontoenv/store.r5tu`` exists, otherwise
  fall back to ``"copy"``.
- ``"rdf5d"`` — zero-copy view backed directly by the on-disk snapshot file. Fastest.
  Raises ``ValueError`` for temporary envs or envs using a custom ``graph_store=``.
- ``"copy"`` — materialize the env's quads into an in-memory snapshot. Works for any
  env kind.

What is supported
-----------------

``OntoEnvStore`` currently supports:

- ``add`` / ``addN``
- ``remove``
- ``triples``
- ``contexts``
- ``len(graph)``
- namespace binding methods such as ``bind`` and ``namespaces``
- SPARQL ``SELECT``, ``ASK``, and graph-producing queries through ``query()``

Current limits:

- SPARQL Update is not implemented.
- The store is currently in-memory only.
- This path does not yet persist into ``rdf5d``.

Query behavior
--------------

SPARQL queries executed through ``rdflib`` are parsed and evaluated in Rust:

- parsing via ``spargebra``
- evaluation via ``spareval``
- results converted back into rdflib ``Result`` objects

That means once you have a graph or dataset backed by ``OntoEnvStore``, normal rdflib
query entrypoints work:

.. code-block:: python

   graph.query("SELECT ?o WHERE { <urn:s> <urn:p> ?o }")
   dataset.query("SELECT ?g ?s WHERE { GRAPH ?g { ?s ?p ?o } }")

``rdflib`` passes graph-selection hints into the store, so dataset-level queries such as
``GRAPH ?g`` and union-style dataset queries work without materializing a second query engine.

Demo script
-----------

See ``python/demo_rdflib_store.py`` for a complete runnable example.
