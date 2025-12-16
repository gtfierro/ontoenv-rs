Python API Reference
====================

This section documents the Python bindings exposed by the ``ontoenv`` package. Regenerate the stubs after changing the Rust layer so that the signatures stay current.

Getting Started
---------------

Install the package from PyPI with ``pip install ontoenv`` (Python 3.9+). The wheels bundle the Rust extension, so you normally do not need a local Rust toolchain.

Example: create an in-memory environment, discover a couple of ontologies from disk, and compute their closure.

.. code-block:: python

   import tempfile
   from pathlib import Path
   from ontoenv import OntoEnv
   from rdflib import Graph

   with tempfile.TemporaryDirectory() as temp_dir:
       root = Path(temp_dir)

       (root / "ontology_a.ttl").write_text(
           """
   @prefix owl: <http://www.w3.org/2002/07/owl#> .
   @prefix : <http://example.com/ontology_a#> .
   <http://example.com/ontology_a> a owl:Ontology .
   """
       )

       (root / "ontology_b.ttl").write_text(
           """
   @prefix owl: <http://www.w3.org/2002/07/owl#> .
   @prefix : <http://example.com/ontology_b#> .
   <http://example.com/ontology_b> a owl:Ontology ;
       owl:imports <http://example.com/ontology_a> .
   """
       )

       env = OntoEnv(
           search_directories=[str(root)],
           strict=False,
           offline=True,
           temporary=True,
       )

       print("Ontologies found:", env.get_ontology_names())

       g = Graph()
       env.get_closure("http://example.com/ontology_b", destination_graph=g)
       print(f"Closure of ontology_b has {len(g)} triples")

       g_a = env.get_graph("http://example.com/ontology_a")
       print(f"Graph of ontology_a has {len(g_a)} triples")

Key Methods
-----------

Some commonly used helpers when scripting with ``OntoEnv``:

- ``OntoEnv(...)`` accepts knobs such as ``search_directories`` (paths to crawl), ``offline`` (skip remote fetches), and ``temporary`` (keep everything in memory).
- ``update(all=False)`` refreshes discovery with the configured directories.
- ``get_closure(name, destination_graph=None, recursion_depth=-1)`` merges the ontology named ``name`` together with the graphs for its imports.
- ``import_dependencies(graph, fetch_missing=False)`` mutates an ``rdflib.Graph`` in place, inserting triples from its declared imports.
- ``get_graph(name)`` returns the stored graph for a specific ontology IRI, which is useful if you only need one ontology rather than a merged closure.

.. toctree::
   :maxdepth: 1

   ontoenv
