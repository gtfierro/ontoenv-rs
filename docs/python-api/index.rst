Python API Reference
====================

This section documents the Python bindings exposed by the ``ontoenv`` package. Regenerate the stubs after changing the Rust layer so that the signatures stay current.

Getting Started
---------------

Install the package from PyPI with ``pip install ontoenv`` (Python 3.9+). The wheels bundle the Rust extension, so you normally do not need a local Rust toolchain.

Example: create an in-memory environment, discover a couple of ontologies from disk, and compute their closure.

.. code-block:: python

   from pathlib import Path
   from ontoenv import OntoEnv
   from rdflib import Graph

   env = OntoEnv(
       search_directories=["."],
       strict=False,
   )
   env.add("https://brickschema.org/schema/1.4.4/Brick.ttl")

   # retrieve a single ontology graph
   brick: Graph = env.get_graph("https://brickschema.org/schema/1.4/Brick")

   # g contains the Brick ontology and all its imports
   g: Graph, imported = env.get_closure("https://brickschema.org/schema/1.4/Brick")
   print(f"Imported {imported} ontologies, total triples: {len(g)}")



Key Methods
-----------

Some commonly used helpers when scripting with ``OntoEnv``:

- ``OntoEnv(...)`` accepts knobs such as ``search_directories`` (paths to crawl), ``offline`` (skip remote fetches), and ``temporary`` (keep everything in memory).
- ``update(all=False)`` refreshes discovery with the configured directories.
- ``add(location, fetch_imports=True)`` accepts a file path, URL, or an in-memory ``rdflib.Graph`` with an ``owl:Ontology`` declaration.
- ``add_no_imports(location)`` accepts the same input types as ``add`` and stores only the root ontology.
- ``get_closure(name, destination_graph=None, recursion_depth=-1)`` merges the ontology named ``name`` together with the graphs for its imports.
- ``import_dependencies(graph, fetch_missing=False)`` mutates an ``rdflib.Graph`` in place, inserting triples from its declared imports.
- ``get_graph(name)`` returns the stored graph for a specific ontology IRI, which is useful if you only need one ontology rather than a merged closure.

.. toctree::
   :maxdepth: 1

   ontoenv
   graph-store
