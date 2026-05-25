Python API Reference
====================

The **ontoenv** Python package exposes the full Rust core through
`PyO3 <https://pyo3.rs>`_ bindings, with native
`rdflib <https://rdflib.readthedocs.io>`_ graph interop.
Pre-built wheels are published on PyPI — no Rust toolchain required.

There are two distinct Python integration surfaces:

- ``OntoEnv`` for ontology discovery, import resolution, and closure materialization.
- ``OntoEnvStore`` for using ontoenv as an ``rdflib`` ``Store`` with Rust-backed SPARQL
  execution.

Install
-------

.. code-block:: bash

   pip install ontoenv   # Python 3.9+

Key methods
-----------

- ``OntoEnv(search_directories, includes, offline, …)`` — Create or open an environment.
  Accepts ``search_directories`` (paths to crawl), ``offline`` (skip network),
  ``temporary`` (keep everything in memory), glob/regex filters, and a custom
  ``graph_store``.
- ``env.update(all=False)`` — Re-run discovery with the configured directories. Pass
  ``all=True`` to force re-fetching of all remote ontologies regardless of cache age.
- ``env.add(location, fetch_imports=True)`` — Register an ontology from a file path, URL,
  or an in-memory ``rdflib.Graph`` that contains an ``owl:Ontology`` declaration. Set
  ``fetch_imports=False`` to store only the root graph.
- ``env.get_closure(name, destination_graph=None, recursion_depth=-1)`` — Return a merged
  ``(Graph, int)`` pair — the ontology named ``name`` plus all its transitive imports, and
  the count of imported graphs. Pass a ``destination_graph`` to merge into an existing graph
  in place.
- ``env.get_graph(name)`` — Return the stored ``rdflib.Graph`` for a single ontology IRI —
  useful when you only need one graph rather than a full closure.
- ``env.import_dependencies(graph, fetch_missing=False)`` — Mutate an ``rdflib.Graph`` in
  place, inserting triples from all ontologies declared in its ``owl:imports`` statements.
  Set ``fetch_missing=True`` to download any imports not yet in the environment.

Example
-------

.. code-block:: python

   from pathlib import Path
   from ontoenv import OntoEnv

   env = OntoEnv(
       search_directories=["./ontologies"],
       includes=["*.ttl"],
       strict=False,
   )

   # Add a remote ontology and follow its imports
   env.add("https://brickschema.org/schema/1.4.4/Brick.ttl")

   # Retrieve just the Brick graph (no imports merged)
   brick = env.get_graph("https://brickschema.org/schema/1.4/Brick")

   # Retrieve Brick with all transitive imports merged
   g, n_imports = env.get_closure("https://brickschema.org/schema/1.4/Brick")
   print(f"Merged {n_imports} imports — {len(g)} triples total")

.. note::

   **Custom storage:** Pass a ``graph_store=`` object to route all graph reads and writes
   through your own backend. This is separate from the built-in ``rdflib`` store integration.
   See `Graph Store Interface <graph-store.html>`_ for the protocol, or
   `RDFLib Store <rdflib-store.html>`_ to query graphs directly through ``rdflib``.

.. toctree::
   :maxdepth: 1

   ontoenv
   rdflib-store
   graph-store
