Python API Reference
====================

The **ontoenv** Python package exposes the full Rust core through
`PyO3 <https://pyo3.rs>`_ bindings, with native
`rdflib <https://rdflib.readthedocs.io>`_ graph interop.
Pre-built wheels are published on PyPI — no Rust toolchain required.

There are two distinct Python integration surfaces:

- ``OntoEnv`` for ontology discovery, import resolution, read-only views, and closure copies.
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
- ``env.get_closure(name, recursion_depth=-1)`` — Return ``(view, closure_names)`` where
  ``view`` is a read-only merged graph view over the ontology named ``name`` plus all its
  transitive imports.
- ``env.copy_closure(name, graph=None, recursion_depth=-1)`` — Copy the ontology named
  ``name`` plus all its transitive imports into a mutable ``rdflib.Graph``. Pass ``graph``
  to merge into an existing graph in place.
- ``env.get_graph(name)`` — Return a read-only store-backed ``rdflib.Graph`` view for a
  single ontology IRI. Cheap; useful when you only need one graph and don't intend to
  mutate it. Mutation raises ``ValueError``.
- ``env.copy_graph(name, graph=None)`` — Materialize a mutable in-memory
  ``rdflib.Graph`` copy of the named ontology. Use this when you need to add or remove
  triples locally without affecting the env.
- ``env.get_dataset()`` — Return a read-only ``rdflib.Dataset`` view of the environment.
- ``env.copy_dataset(dataset=None)`` — Copy the environment into a mutable
  ``rdflib.Dataset``.
- ``env.iter_triples(name)`` / ``env.iter_closure_triples(name, recursion_depth=-1)`` —
  Streaming iterators of ``(s, p, o)`` rdflib-term tuples for a single ontology or its
  imports closure. Skip the ``rdflib.Graph`` wrapper entirely; closure iteration does
  *not* de-duplicate across named graphs.
- ``env.import_dependencies(graph, fetch_missing=False)`` — Mutate an ``rdflib.Graph`` in
  place, inserting triples from all ontologies declared in its ``owl:imports`` statements.
  Set ``fetch_missing=True`` to download any imports not yet in the environment.
- ``env.build_index()`` / ``env.set_auto_index(on)`` — Manage the PSO/POS sidecar index
  (``store.r5tu.idx``) that accelerates triple-pattern queries with a bound predicate.
  Built automatically on every flush by default; pass ``auto_index=False`` to the
  constructor to disable. See `Benchmarks <benchmarks.html#pso-pos-sidecar-index>`_.

Pythonic sugar
~~~~~~~~~~~~~~

``OntoEnv`` supports the standard container and context-manager protocols:

- ``len(env)`` — number of ontologies in the environment.
- ``uri in env`` — ``True`` if *uri* resolves to a known ontology (canonical name, alias, or source URL).
- ``env[uri]`` — shorthand for ``env.get_graph(uri)``.
- ``for name in env: ...`` — iterate over the URIs of every ontology in the environment.
- ``with OntoEnv(...) as env: ...`` — automatically persist (where applicable) and release resources on exit.

.. code-block:: python

   with OntoEnv(path="./.ontoenv") as env:
       env.add("https://brickschema.org/schema/1.4.4/Brick.ttl")

       print(f"{len(env)} ontologies")
       for name in env:
           print(name, len(env[name]))

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

   # Query Brick with all transitive imports through a read-only merged view
   g, closure_names = env.get_closure("https://brickschema.org/schema/1.4/Brick")
   print(f"Read {len(closure_names)} graphs — {len(g)} triples total")

   # Copy the same closure when you need a mutable materialized graph
   mutable_g, closure_names = env.copy_closure("https://brickschema.org/schema/1.4/Brick")

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
   benchmarks
