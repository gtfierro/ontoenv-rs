OntoEnv from Python
===================

This tutorial builds the same environment as :doc:`first-environment` from
Python, then runs a SPARQL query over its resolved imports closure. It does
not depend on the CLI tutorial.

.. raw:: html

   <div class="oe-minimum">
     <h3>Basic workflow</h3>
     <p>This tutorial uses four calls:</p>
   </div>

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv.connect(                     # 1. open (creating if needed)
       "./ontology-env",
       search_directories=["./ontologies"],
   )
   env.update()                               # 2. scan the source directory
   view, imported = env.get_closure(          # 3. read a resolved closure
       "https://example.org/site"
   )
   env.close()                                # 4. release resources

The rest of the tutorial explains the state each call creates or reads.

Install
-------

.. code-block:: bash

   pip install ontoenv    # Python 3.11+

Wheels are pre-built, so no Rust toolchain is required. You will also want
``rdflib``, which comes along as a dependency.

Create some ontologies
----------------------

Make a working directory with two ontology files:

.. code-block:: bash

   mkdir -p tutorial/ontologies
   cd tutorial

``ontologies/sensors.ttl``:

.. code-block:: turtle

   @prefix owl:  <http://www.w3.org/2002/07/owl#> .
   @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
   @prefix sen:  <https://example.org/sensors#> .

   <https://example.org/sensors> a owl:Ontology .

   sen:Sensor       a owl:Class ; rdfs:label "Sensor" .
   sen:Thermometer  a owl:Class ; rdfs:subClassOf sen:Sensor .

``ontologies/site.ttl``:

.. code-block:: turtle

   @prefix owl:  <http://www.w3.org/2002/07/owl#> .
   @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
   @prefix sen:  <https://example.org/sensors#> .
   @prefix site: <https://example.org/site#> .

   <https://example.org/site> a owl:Ontology ;
       owl:imports <https://example.org/sensors> .

   site:Room a owl:Class ; rdfs:label "Room" .
   site:hasSensor a owl:ObjectProperty ;
       rdfs:domain site:Room ;
       rdfs:range sen:Sensor .

``site.ttl`` imports ``https://example.org/sensors`` by IRI. Nothing in the
file says where that ontology lives; resolving that is OntoEnv's job.

Connect to an environment
-------------------------

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv.connect(
       "./ontology-env",
       search_directories=["./ontologies"],
   )
   env.update()

   print(env.get_ontology_names())
   # ['https://example.org/sensors', 'https://example.org/site']

These calls have separate responsibilities:

``connect`` opens the environment at ``./ontology-env``, creating it if it is
not there. It reads the saved catalog but does not read the ontology files.

``update`` is what scans ``search_directories`` for new and changed files,
parses them, and follows their imports. Keeping these separate means restarting
your program does not re-read every file on disk.

On a later run, ``connect`` reopens the saved environment. ``update`` checks
the sources and does no parsing if nothing has changed.

Read a single graph
-------------------

.. code-block:: python

   g = env.get_graph("https://example.org/site")
   print(len(g))   # just the triples in site.ttl

``get_graph`` returns a read-only ``rdflib.Graph`` backed by OntoEnv's
storage. It does not copy the graph. Adding or removing triples raises
``ValueError``. Use ``copy_graph`` when the caller needs to modify it:

.. code-block:: python

   from rdflib import Literal, URIRef

   g = env.copy_graph("https://example.org/site")
   g.add((URIRef("https://example.org/site#Room"),
          URIRef("http://www.w3.org/2000/01/rdf-schema#comment"),
          Literal("A room")))

This read-only-by-default, copy-on-request split runs through the whole API:
every ``get_*`` method returns a view, and every ``copy_*`` method materializes
a mutable ``rdflib`` object. :doc:`../explanation/views-and-copies` covers when
to use each one.

Resolve the imports closure
---------------------------

.. code-block:: python

   view, imported = env.get_closure("https://example.org/site")

   print(imported)
   # ['https://example.org/site', 'https://example.org/sensors']
   print(len(view))   # triples from both graphs, merged

``get_closure`` returns two things: a read-only view over the ontology plus
all its transitive imports, and the list of graphs that went into it.

The view is not a raw concatenation. Resolved ``owl:imports`` statements are
removed, ontology declarations from imported graphs are collapsed onto the
root, and duplicates appear once. The resulting graph can be passed to a
consumer without requiring it to resolve the original imports again.

As with single graphs, ``copy_closure`` gives you the same content as a
mutable ``rdflib.Graph``:

.. code-block:: python

   g, imported = env.copy_closure("https://example.org/site")
   g.serialize("closure.ttl", format="turtle")

Query it with SPARQL
--------------------

The view supports ``query()`` directly, and the query executes in Rust against
OntoEnv's storage rather than in rdflib's Python engine:

.. code-block:: python

   view, _ = env.get_closure("https://example.org/site")

   rows = view.query("""
       PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
       PREFIX owl:  <http://www.w3.org/2002/07/owl#>
       SELECT ?cls ?label WHERE {
           ?cls a owl:Class .
           OPTIONAL { ?cls rdfs:label ?label }
       }
   """)

   for row in rows:
       print(row.cls, row.label)

The query sees classes from *both* files, because the closure merged them.
The query is scoped to the resolved closure, so it sees classes from both
source files without application code traversing the import graph.

Add an ontology from the web
----------------------------

.. code-block:: python

   name = env.add("https://brickschema.org/schema/1.4.4/Brick.ttl")
   print(name)   # 'https://brickschema.org/schema/1.4/Brick'

   view, imported = env.get_closure(name)
   print(f"{len(imported)} graphs, {len(view)} triples")

``add`` fetches the URL, follows its ``owl:imports``, fetches those, and
returns the ontology's canonical IRI — which, as here, is often not the same
as the URL you downloaded it from. Remote copies are cached on disk.

Close the environment
---------------------

.. code-block:: python

   env.close()

Or let a ``with`` block do it:

.. code-block:: python

   with OntoEnv.connect("./ontology-env") as env:
       view, imported = env.get_closure("https://example.org/site")
       print(len(view))

The context manager is a convenience for scripts, not a requirement. A
long-running server should connect once at startup and close at shutdown —
see :doc:`../how-to/use-in-a-service`.

Skip persistence entirely
-------------------------

For a notebook or a test where nothing should be written to disk:

.. code-block:: python

   env = OntoEnv(temporary=True)
   env.add("./ontologies/sensors.ttl", fetch_imports=False)
   env.add("./ontologies/site.ttl")

The first ``add`` registers the graph that ``site.ttl`` imports. The second can
therefore resolve that import without accessing the network. Both graphs and
the dependency index remain in memory; no ``.ontoenv/`` directory is written.

What the example did
--------------------

- ``OntoEnv.connect(path)`` opens or creates a persistent environment;
  ``update()`` is the separate, explicit step that reads source files.
- ``get_*`` returns store-backed read-only views; ``copy_*`` returns mutable
  ``rdflib`` objects.
- ``get_closure`` merges an ontology with its transitive imports into one
  flattened graph, and that view answers SPARQL queries directly.
- ``OntoEnv(temporary=True)`` gives you the same API with nothing persisted.

Next steps
----------

- :doc:`../how-to/use-in-a-service` — connecting once and sharing the
  environment across requests.
- :doc:`../how-to/query-with-sparql` — using the environment as an ``rdflib``
  store and querying across named graphs.
- :doc:`../reference/python` — every method, grouped by what it does.
