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
- ``env.add(location, fetch_imports=True, rename=None)`` — Register an ontology from a file
  path, URL, or an in-memory ``rdflib.Graph``.  Set ``fetch_imports=False`` to skip
  ``owl:imports`` traversal.  Pass ``rename="<new-iri>"`` to override the ontology IRI
  stored in the environment — the declared IRI in the graph is rewritten to the new value
  (see :ref:`renaming-on-add` in the CLI docs for what gets rewritten).
- ``env.add_no_imports(location, rename=None)`` — Same as ``add`` but always skips import
  traversal.  Also accepts ``rename``.
- ``env.rename_graph_iri(old_iri, new_iri)`` — Rename an ontology already in the
  environment.  Reads the stored graph, rewrites all occurrences of ``old_iri`` to
  ``new_iri`` (subject and object positions, excluding ``owl:versionIRI`` values), and
  updates the import dependency graph.  Returns the new IRI.

**Read-only views** vs **mutable copies**:

- ``env.get_graph(name)`` — Return a read-only store-backed ``rdflib.Graph`` view for a
  single ontology IRI. Mutation raises ``ValueError``; use ``copy_graph`` when you need
  to add or remove triples locally.
- ``env.copy_graph(name, graph=None)`` — Materialize a mutable in-memory
  ``rdflib.Graph`` copy of the named ontology.
- ``env.get_closure(name, recursion_depth=-1)`` — Return ``(view, closure_names)`` where
  ``view`` is a read-only ``ClosureGraphView`` over the ontology named ``name`` plus all
  its transitive imports. No triples are materialized; use ``copy_closure`` for a mutable
  copy.
- ``env.copy_closure(name, graph=None, recursion_depth=-1)`` — Copy the ontology named
  ``name`` plus all its transitive imports into a mutable ``rdflib.Graph``. Pass ``graph``
  to merge into an existing graph in place.
- ``env.get_union(uris, include_closures=False, recursion_depth=-1)`` — Return
  ``(view, graph_iris)`` where ``view`` is a read-only ``ClosureGraphView`` over an
  explicitly listed set of graphs. Set ``include_closures=True`` to expand each listed
  graph's transitive ``owl:imports`` closure. No triples are materialized; use
  ``copy_union`` for a mutable copy.
- ``env.copy_union(uris, root, graph=None, include_closures=False, …)`` — Materialize the
  union of explicitly listed graphs into a mutable ``rdflib.Graph``. ``root`` drives
  ontology-declaration cleanup and optional SHACL prefix rewriting.
- ``env.get_dataset()`` — Return a read-only ``rdflib.Dataset`` view of the environment.
- ``env.copy_dataset(dataset=None)`` — Copy the environment into a mutable
  ``rdflib.Dataset``.
- ``env.iter_triples(name)`` / ``env.iter_closure_triples(name, recursion_depth=-1)`` —
  Streaming iterators of ``(s, p, o)`` rdflib-term tuples for a single ontology or its
  imports closure. Skip the ``rdflib.Graph`` wrapper entirely; closure iteration does
  *not* de-duplicate across named graphs.
- ``env.import_dependencies(graph, fetch_missing=False)`` — Mutate an ``rdflib.Graph`` in
  place, inserting triples from all ontologies declared in its ``owl:imports`` statements.

**Alias management** — route multiple IRIs to a single canonical ontology:

- ``env.add_alias(alias_iri, canonical_iri)`` — Create an alias that resolves to the same
  graph as the canonical IRI.  Aliases can only point to canonical IRIs (not other
  aliases), preventing chains.
- ``env.remove_alias(alias_iri)`` — Remove a previously registered alias.
- ``env.resolve_alias(alias_iri)`` — Return the canonical IRI for an alias, or ``None``.
- ``env.get_aliases_for(canonical_iri)`` — List all aliases that point to a given canonical
  IRI.
- ``env.is_canonical_iri(iri)`` — Check whether an IRI is a canonical ontology (not an
  alias).  Aliases, container/context-manager protocols, and ``get_closure`` all
  transparently resolve aliases to the canonical graph.

**Configuration** at runtime (these are also available as CLI flags at init time):

- ``env.set_offline(bool)`` / ``env.is_offline()`` — Toggle or query offline mode.
- ``env.set_strict(bool)`` / ``env.is_strict()`` — Strict mode makes missing imports an
  error instead of a warning.
- ``env.set_remote_cache_ttl_secs(secs)`` — Override the remote-ontology cache TTL.
- ``env.set_use_cached_ontologies(mode)`` — Control whether cached copies or fresh fetches
  are preferred.

Query indexes (PSO/POS/SPO/OSP posting lists and a transitive-closure table for property
paths) are built in memory on first use — no setup, no on-disk sidecar. See
`Benchmarks <benchmarks.html#in-memory-query-indexes>`_.

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
