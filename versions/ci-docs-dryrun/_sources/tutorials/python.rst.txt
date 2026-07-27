OntoEnv from Python
===================

This tutorial builds the same environment as :doc:`first-environment`, but
from Python, and ends by running a SPARQL query against a resolved imports
closure.

About 10 minutes. You do not need to have done the CLI tutorial first.

.. raw:: html

   <div class="oe-minimum">
     <h3>The minimum you need to know</h3>
     <p>Four calls cover almost every use of the Python API:</p>
   </div>

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv.connect("./ontology-env")   # 1. open (creating it if needed)
   name = env.add("./ontologies/site.ttl")   # 2. register an ontology
   view, imported = env.get_closure(name)    # 3. read it plus its imports
   env.close()                               # 4. release resources

Everything below is that, slowed down.

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

Two calls, two distinct jobs:

``connect`` opens the environment at ``./ontology-env``, creating it if it is
not there. It is deliberately cheap — it does not read your ontology files.

``update`` is what scans ``search_directories`` for new and changed files,
parses them, and follows their imports. Keeping these separate means restarting
your program does not re-read every file on disk.

Run the script a second time. ``connect`` reopens the saved environment
directly, and ``update`` finds nothing new to do.

Read a single graph
-------------------

.. code-block:: python

   g = env.get_graph("https://example.org/site")
   print(len(g))   # just the triples in site.ttl

``get_graph`` returns a read-only ``rdflib.Graph`` backed by OntoEnv's
storage. Nothing is copied, so this is fast even for large ontologies — but
adding or removing triples raises ``ValueError``. When you want a graph you
can edit, ask for a copy:

.. code-block:: python

   from rdflib import Literal, URIRef

   g = env.copy_graph("https://example.org/site")
   g.add((URIRef("https://example.org/site#Room"),
          URIRef("http://www.w3.org/2000/01/rdf-schema#comment"),
          Literal("A room")))

This read-only-by-default, copy-on-request split runs through the whole API:
every ``get_*`` method returns a view, and every ``copy_*`` method materializes
a mutable ``rdflib`` object. :doc:`../explanation/views-and-copies` covers when
each is the right choice.

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
stripped out, the imported graphs' ontology declarations are collapsed onto
the root, and duplicate triples appear once. It is a single flattened graph
that stands on its own.

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
That is the point: you write one query against one graph and the import
resolution has already happened.

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
   env.add("./ontologies/site.ttl")

A temporary environment keeps everything in memory and leaves no files behind.
It also has no saved index, so it starts from nothing every time.

What you learned
----------------

- ``OntoEnv.connect(path)`` opens or creates a persistent environment;
  ``update()`` is the separate, explicit step that reads source files.
- ``get_*`` returns fast read-only views; ``copy_*`` returns mutable
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
