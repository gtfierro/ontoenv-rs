Python API Reference
====================

The **ontoenv** Python package exposes the full Rust core through
`PyO3 <https://pyo3.rs>`_ bindings, with native
`rdflib <https://rdflib.readthedocs.io>`_ graph interop.
Pre-built wheels are published on PyPI — no Rust toolchain required.

There are two Python integration surfaces:

- ``OntoEnv`` for ontology discovery, import resolution, read-only views, and closure copies.
- ``OntoEnvStore`` for using ontoenv as an ``rdflib`` ``Store`` with Rust-backed SPARQL
  execution.

Install
-------

.. code-block:: bash

   pip install ontoenv   # Python 3.11+

First steps
-----------

Use ``connect`` for a persistent environment:

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv.connect("./ontology-env")
   ontology = env.add("./ontologies/site.ttl")

   graph = env.get_graph(ontology)
   closure, imported = env.get_closure(ontology)

   env.close()

Use ``OntoEnv(temporary=True)`` when nothing should be saved. See
:doc:`lifecycle` for scripts, webservers, custom stores, synchronization, and
the stricter lifecycle methods.

What OntoEnv keeps track of
---------------------------

As ontologies are added, OntoEnv records their canonical names, source URLs,
aliases, namespace declarations, and ``owl:imports`` relationships. You can
therefore ask for one graph, follow its complete imports closure, or find which
ontologies depend on it without rebuilding those relationships yourself.

Read methods return store-backed views so large graphs do not have to be
copied. When a caller needs to edit or export an independent graph, the
corresponding ``copy_*`` method creates a mutable copy. The same API works with
an in-memory temporary environment, OntoEnv's persistent storage, or a custom
graph store.

Persistent environments save the ontology information needed for fast
restarts. If a custom store changes elsewhere, OntoEnv can incorporate the
affected graphs when the store identifies them, while leaving a full scan as
an explicit choice.

Key methods
-----------

- ``OntoEnv.connect(path, sync="auto", …)`` — Recommended persistent entry point.
  It handles first use, fast restarts, and direct graph-store changes. It does
  not refresh ontology files or URLs; see :ref:`refreshing-ontology-sources`.
- ``OntoEnv.create(path, …)`` — Require a new persistent environment.
- ``OntoEnv.open(path, read_only=False, …)`` — Load an existing saved environment
  without scanning ontology graphs.
- ``OntoEnv.adopt(path, graph_store, …)`` — Deliberately scan a pre-populated custom
  store and save its first ontology index.
- ``OntoEnv(..., temporary=True)`` — Keep graphs and ontology information in memory.
  See :doc:`lifecycle` for API selection, long-lived webserver usage, and cleanup.
- ``env.update()`` — Refresh changed known sources and follow their imports.
  Pass a file or URL to update one source, or ``force=True`` to reread sources
  regardless of timestamps and cache age.
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
  ``rdflib.Graph`` copy of the named ontology. A currently known unresolved
  import raises ``UnresolvedImportError``; an arbitrary unknown IRI raises
  ``ValueError``.
- ``env.get_closure(name, recursion_depth=-1)`` — Return ``(view, closure_names)`` where
  ``view`` is a read-only :py:class:`ontoenv.ViewGraph` over the ontology named ``name``
  plus all its transitive imports. The view presents a single flattened, de-duplicated
  graph with the **same triple set as** ``copy_closure`` (resolved ``owl:imports``
  stripped, ontology declarations collapsed onto the root, SHACL prefixes consolidated).
  Persistent local environments use a zero-copy mmap view; temporary and custom-store
  environments use a normalized in-memory read snapshot.
- ``env.copy_closure(name, graph=None, recursion_depth=-1)`` — Materialize the same
  cleaned, flattened closure into a mutable ``rdflib.Graph``. Pass ``graph`` to merge into
  an existing graph in place.
- ``env.get_union(uris, include_closures=False, recursion_depth=-1)`` — Return
  ``(view, graph_iris)`` where ``view`` is a read-only :py:class:`ontoenv.ViewGraph`
  over an explicitly listed set of graphs. Unlike ``get_closure`` this is a **raw** merge:
  no closure transform is applied and triples are not de-duplicated across graphs. Set
  ``include_closures=True`` to expand each listed graph's transitive ``owl:imports``
  closure. No triples are materialized; use ``copy_union`` for a mutable copy.
- ``env.copy_union(uris, root, graph=None, include_closures=False, …)`` — Materialize the
  union of explicitly listed graphs into a mutable ``rdflib.Graph``. Defaults to a raw
  union; pass ``rewrite_sh_prefixes=True`` / ``remove_owl_imports=True`` to opt into the
  closure transforms, with ``root`` driving ontology-declaration and SHACL-prefix cleanup.
- ``env.get_dataset()`` — Return a read-only ``rdflib.Dataset`` view of the environment.
- ``env.copy_dataset(dataset=None)`` — Copy the environment into a mutable
  ``rdflib.Dataset``.
- ``env.iter_triples(name)`` / ``env.iter_closure_triples(name, recursion_depth=-1)`` —
  Streaming iterators of ``(s, p, o)`` rdflib-term tuples for a single ontology or its
  imports closure. Skip the ``rdflib.Graph`` wrapper entirely; closure iteration does
  *not* de-duplicate across named graphs.
- ``env.import_dependencies(graph, fetch_missing=False)`` — Mutate an ``rdflib.Graph`` in
  place, inserting triples from available ontologies declared in its ``owl:imports``
  statements. With ``fetch_missing=True``, non-strict mode records and skips unavailable
  targets without leaving a recovery marker.
- ``env.get_dependencies(graph, graph_name=None, fetch_missing=False)`` — Perform the same
  dependency resolution without modifying the caller's graph. It has the same
  strict/non-strict and recovery-marker behavior as ``import_dependencies``.

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
- ``env.set_require_ontology_names(bool)`` / ``env.requires_ontology_names()`` —
  Require an ontology declaration during future ingestion.
- ``env.set_remote_cache_ttl_secs(secs)`` / ``env.remote_cache_ttl_secs()`` —
  Override or query the remote-ontology cache TTL.
- ``env.set_use_cached_ontologies(enabled)`` / ``env.uses_cached_ontologies()`` —
  Control whether cached copies or fresh fetches
  are preferred.
- ``env.set_resolution_policy(name)`` / ``env.resolution_policy()`` — Select or
  query ``"default"``, ``"latest"``, or ``"version"`` resolution.
- ``OntoEnv.recover(path, graph_store=None)`` — Rebuild the catalog after
  ``CatalogRecoveryError`` without manually deleting OntoEnv-owned files.

When reopening, every configuration option follows the same rule: omission
preserves its persisted value, while an explicit value (including ``False``,
``"default"``, or ``[]``) overrides it. Writable opens persist overrides and
read-only opens keep them session-local. Reconfiguration does not implicitly
scan or re-ingest; changed search paths and filters apply on the next
``update()``.

A known unresolved ``owl:imports`` target passed to ``copy_graph`` raises
``UnresolvedImportError`` (a ``LookupError``). Known targets include imports
declared by catalogued ontologies and currently recorded targets encountered while
fetching dependencies for a transient graph, including indirect imports. This lets
consumers catch one type for expected missing imports while parsing, storage, and
other failures retain their own error paths. An arbitrary unknown graph IRI that
was never declared or attempted remains a normal lookup ``ValueError``.

PSO/POS/SPO/OSP posting-list indexes are built in memory when an rdf5d snapshot is bound;
the transitive-closure table for supported property paths is built lazily on first use.
There is no setup and no on-disk sidecar. See
`Benchmarks <benchmarks.html#in-memory-query-indexes>`_.

Pythonic sugar
~~~~~~~~~~~~~~

``OntoEnv`` supports the standard container and context-manager protocols.
The context manager is optional; long-lived applications may retain the object
and call ``close()`` from their shutdown hook:

- ``len(env)`` — number of ontologies in the environment.
- ``uri in env`` — ``True`` if *uri* resolves to a known ontology (canonical name, alias, or source URL).
- ``env[uri]`` — shorthand for ``env.get_graph(uri)``.
- ``for name in env: ...`` — iterate over the URIs of every ontology in the environment.
- ``with OntoEnv.open(...) as env: ...`` — automatically close and release resources on exit.

.. code-block:: python

   with OntoEnv.open("./environment") as env:
       env.add("https://brickschema.org/schema/1.4.4/Brick.ttl")

       print(f"{len(env)} ontologies")
       for name in env:
           print(name, len(env[name]))

.. code-block:: python

   # Equivalent direct lifetime management for a service:
   env = OntoEnv.connect("./environment")
   application_state.ontoenv = env
   # Call application_state.ontoenv.close() during application shutdown.

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
   lifecycle
   rdflib-store
   graph-store
   benchmarks
