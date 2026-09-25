Query with SPARQL
=================

OntoEnv evaluates SPARQL in Rust, reading directly from its on-disk storage.
You get to that engine through ordinary ``rdflib`` entry points. Pick the
scope you need:

.. list-table::
   :header-rows: 1
   :widths: 34 33 33

   * - You want to query
     - Use
     - Returns
   * - One ontology plus its imports
     - ``env.get_closure(iri)``
     - a read-only ``ViewGraph``
   * - An explicit set of graphs
     - ``env.get_union(iris)``
     - a read-only ``ViewGraph``
   * - The whole environment, named graphs intact
     - ``env.get_dataset()``
     - a read-only ``rdflib.Dataset``

Query an imports closure
------------------------

.. code-block:: python

   view, imported = env.get_closure("https://brickschema.org/schema/1.4/Brick")

   rows = view.query("""
       PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
       PREFIX brick: <https://brickschema.org/schema/Brick#>
       SELECT ?sub WHERE { ?sub rdfs:subClassOf* brick:Equipment }
   """)

   for row in rows:
       print(row.sub)

The query is scoped to the graphs in the closure and sees them as one
flattened graph. Nothing is materialized in Python.

Recursive property paths on ``rdfs:subClassOf``, ``rdfs:subPropertyOf``, and
``owl:sameAs`` are answered from a precomputed transitive-closure table, which
is why the query above is fast. See :doc:`../explanation/performance`.

Query across named graphs
-------------------------

When you need to know *which* graph a triple came from, use the dataset view:

.. code-block:: python

   dataset = env.get_dataset()

   rows = dataset.query("""
       SELECT ?g (COUNT(*) AS ?triples) WHERE {
           GRAPH ?g { ?s ?p ?o }
       } GROUP BY ?g
   """)

   for row in rows:
       print(row.g, int(row.triples))

Each named graph is keyed by its ontology IRI, and the namespaces OntoEnv
knows about are already bound. To pull one graph out of the dataset:

.. code-block:: python

   from rdflib import URIRef

   brick = dataset.graph(URIRef("https://brickschema.org/schema/1.4/Brick"))
   print(len(brick))

A dataset reflects the environment as of the moment you asked for it. After
mutating the environment, ask again or refresh in place:

.. code-block:: python

   env.add("./ontologies/new.ttl")
   env.flush()
   env.refresh_dataset(dataset)

Query a set of graphs you choose
--------------------------------

.. code-block:: python

   view, graph_iris = env.get_union([
       "https://example.org/a",
       "https://example.org/b",
   ])

   # Expand each listed graph's transitive imports too
   view, graph_iris = env.get_union(
       ["https://example.org/a"],
       include_closures=True,
   )

Unlike ``get_closure``, a union is a **raw** merge: no import stripping, no
ontology-declaration collapsing, and no de-duplication across graphs. Use it
when you want exactly the graphs you named and nothing done to them.

Use the rdflib plugin
---------------------

Importing ``ontoenv`` registers an ``rdflib`` store plugin named ``"ontoenv"``:

.. code-block:: python

   from rdflib import Graph
   import ontoenv   # registers the plugin

   graph = Graph(store="ontoenv")

This is useful when a library you do not control constructs graphs by store
name.

Query from the command line
---------------------------

The CLI has no ``query`` subcommand. Export the graph you want and query it
with your usual tooling:

.. code-block:: console

   $ ontoenv closure https://example.org/site closure.ttl

What is not supported
---------------------

- **SPARQL Update.** The exposed store is a read-only snapshot. Mutate the
  environment through ``OntoEnv`` methods, then take a fresh view.
- **Writing through the store.** ``add``, ``addN``, and ``remove`` raise
  ``ValueError`` on both ``ViewGraph`` and ``OntoEnvStore``.

For a mutable graph you can query with rdflib's own engine, use ``copy_closure``
or ``copy_dataset``.

.. seealso::

   :doc:`../reference/rdflib-store` for the full ``ViewGraph`` and
   ``OntoEnvStore`` surface, and ``python/demo_rdflib_store.py`` in the
   repository for a runnable example.
