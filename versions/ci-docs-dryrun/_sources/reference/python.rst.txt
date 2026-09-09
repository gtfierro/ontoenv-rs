Python API reference
====================

.. code-block:: bash

   pip install ontoenv    # Python 3.11+

The package exposes the Rust core through `PyO3 <https://pyo3.rs>`_ bindings,
with native `rdflib <https://rdflib.readthedocs.io>`_ interop. Pre-built
wheels are published on PyPI; no Rust toolchain is required.

This page groups the API by purpose. :doc:`api` has the generated signatures
and docstrings.

Opening an environment
----------------------

.. list-table::
   :header-rows: 1
   :widths: 42 58

   * - Call
     - Behavior
   * - ``OntoEnv.connect(path, *, graph_store=None, sync="auto", read_only=False, **options)``
     - Create if missing, reopen if present. The normal entry point.
   * - ``OntoEnv.create(path, *, graph_store=None, **options)``
     - Create a new environment; fails if one exists unless
       ``overwrite=True``.
   * - ``OntoEnv.open(path, *, graph_store=None, read_only=False, **options)``
     - Open an existing environment; fails if missing. Never synchronizes.
   * - ``OntoEnv.adopt(path, graph_store, *, overwrite=False, **options)``
     - Index an already-populated custom store for the first time.
   * - ``OntoEnv.recover(path, *, graph_store=None, **options)``
     - Rebuild the catalog after ``CatalogRecoveryError``.
   * - ``OntoEnv(temporary=True, **options)``
     - In-memory environment; nothing is persisted.

``sync`` accepts ``"auto"`` (default), ``"full"``, or ``"catalog"`` — see
:doc:`../explanation/staying-in-sync`.

``**options`` accepts any key from :doc:`configuration`. On reopen, an omitted
option preserves its saved value; an explicit value — including ``False``,
``"default"``, and ``[]`` — overrides it. Writable connections persist
overrides; read-only ones keep them session-local.

.. rubric:: Direct constructor

.. code-block:: python

   OntoEnv(path=None, recreate=False, read_only=False, temporary=False,
           search_directories=None, includes=None, excludes=None,
           include_ontologies=None, exclude_ontologies=None,
           strict=None, offline=None, require_ontology_names=None,
           use_cached_ontologies=None, resolution_policy=None,
           remote_cache_ttl_secs=None, graph_store=None, root=".")

Supported and used internally, but the named methods above express intent more
clearly. Note that ``recreate=True`` **deletes and rebuilds** the target
``.ontoenv`` directory — it is not a reconnect.

.. deprecated:: 0.6
   ``create_or_use_cached=True`` emits ``DeprecationWarning``; use
   ``OntoEnv.connect(path)``. Removal planned for 0.7.

.. deprecated:: 0.6
   ``init_from_store=True``; use ``OntoEnv.adopt(path, graph_store)``.

Closing
~~~~~~~

- ``env.close()`` — release resources.
- ``env.flush()`` — write pending changes to storage.
- ``with OntoEnv.connect(...) as env:`` — calls ``close()`` on exit.

Adding ontologies
-----------------

.. list-table::
   :header-rows: 1
   :widths: 46 54

   * - Method
     - Returns / notes
   * - ``add(location, overwrite=False, fetch_imports=True, force=False, rename=None)``
     - The ontology's IRI. *location* is a path, URL, or ``rdflib.Graph``.
   * - ``add_no_imports(location, overwrite=False, force=False, rename=None)``
     - As ``add`` but never follows ``owl:imports``.
   * - ``rename_graph_iri(uri, new_iri)``
     - The new IRI. Rewrites the stored graph and rebuilds the import graph.

``rename=`` rewrites every occurrence of the declared IRI in the stored graph,
except ``owl:versionIRI`` values. See :doc:`../how-to/rename-and-alias`.

Refreshing
----------

.. list-table::
   :header-rows: 1
   :widths: 46 54

   * - Method
     - Reconciles
   * - ``update(location=None, *, force=False)``
     - Ontology **sources** — files and URLs — following their imports.
       Without *location*, all configured sources.
   * - ``refresh_from_store(graphs=None, full=False)``
     - Graphs changed **directly in a custom store**. Returns a
       ``SyncReport`` with ``added``, ``changed``, ``removed``.

``graphs=`` is an exact set of backend graph IDs and is never expanded.
``full=True`` cannot be combined with it.

.. deprecated:: 0.6
   ``update(all=True)``; use ``update(force=True)``.

Reading graphs
--------------

Every read comes in a read-only view and a mutable copy. See
:doc:`../explanation/views-and-copies`.

.. list-table::
   :header-rows: 1
   :widths: 40 60

   * - Method
     - Returns
   * - ``get_graph(uri)``
     - Read-only store-backed ``rdflib.Graph``. Mutation raises
       ``ValueError``.
   * - ``copy_graph(uri, graph=None)``
     - Mutable ``rdflib.Graph``. Raises ``UnresolvedImportError`` for a known
       unresolved import, ``ValueError`` for an unknown IRI.
   * - ``get_closure(uri, recursion_depth=-1, remove_owl_imports=True, rewrite_sh_prefixes=True)``
     - ``(ViewGraph, closure_names)`` — flattened, de-duplicated view over the
       ontology and its transitive imports.
   * - ``copy_closure(uri, graph=None, rewrite_sh_prefixes=True, remove_owl_imports=True, recursion_depth=-1)``
     - ``(Graph, closure_iris)`` — same triple set, materialized and mutable.
   * - ``get_union(uris, include_closures=False, recursion_depth=-1)``
     - ``(ViewGraph, graph_iris)`` — **raw** merge of the listed graphs; no
       transform, no cross-graph de-duplication.
   * - ``copy_union(uris, root, graph=None, include_closures=False, rewrite_sh_prefixes=False, remove_owl_imports=False, recursion_depth=-1)``
     - ``(Graph, graph_iris)`` — raw by default; pass the transform flags to
       opt in, with *root* driving declaration and prefix cleanup.
   * - ``get_dataset()``
     - Read-only ``rdflib.Dataset`` view of the whole environment.
   * - ``copy_dataset(dataset=None)``
     - Mutable ``rdflib.Dataset`` copy.
   * - ``refresh_dataset(dataset)``
     - Re-snapshot the environment into an existing store-backed dataset.

.. deprecated:: 0.6
   ``snapshot_as_dataset(...)`` and ``to_rdflib_dataset(...)``; use
   ``get_dataset()`` / ``copy_dataset()``.

Streaming
~~~~~~~~~

- ``iter_triples(uri)`` — ``(s, p, o)`` rdflib terms for one graph.
- ``iter_closure_triples(uri, recursion_depth=-1)`` — the same across a
  closure. **Not** de-duplicated across named graphs.

Merging into a caller's graph
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

- ``import_graph(destination_graph, uri, recursion_depth=-1)`` — merge the
  closure of *uri* into *destination_graph* in place.
- ``import_dependencies(graph, recursion_depth=-1, fetch_missing=False)`` —
  resolve *graph*'s own ``owl:imports`` and merge them into it. Returns the
  merged IRIs.
- ``get_dependencies(graph, graph_name=None, recursion_depth=-1, fetch_missing=False)`` —
  ``(Graph, closure_iris)``; same resolution without modifying the caller's
  graph. *graph_name* overrides the IRI used for ``sh:prefixes`` rewriting.

With ``fetch_missing=True``, strict mode aborts on an unavailable import while
non-strict mode records and skips it. A completed best-effort call leaves no
recovery marker.

Inspecting
----------

.. list-table::
   :header-rows: 1
   :widths: 46 54

   * - Method
     - Returns
   * - ``get_ontology_names()``
     - Every ontology IRI in the environment.
   * - ``get_ontology(uri)``
     - An ``Ontology`` metadata object.
   * - ``get_importers(uri)``
     - IRIs that *directly* import *uri*.
   * - ``list_closure(uri, recursion_depth=-1)``
     - Closure IRIs. *uri* may be a string IRI or an ``rdflib.Graph`` not yet
       in the environment.
   * - ``missing_imports(uri=None)``
     - Unresolvable ``owl:imports`` targets. ``None`` covers the whole
       environment; a string IRI walks that ontology's closure; a ``Graph``
       checks its direct imports.
   * - ``get_namespaces(ontology=None, include_closure=False)``
     - Prefix → namespace mapping.
   * - ``store_path()``
     - Filesystem path of the graph store, if any.
   * - ``dump(includes=None)``
     - Print the environment state to stdout.

``Ontology`` exposes ``id``, ``name``, ``imports``, ``location``,
``last_updated``, ``version_properties``, and ``namespace_map``.

Aliases
-------

- ``add_alias(alias_iri, canonical_iri)`` — an alias may only point at a
  canonical IRI, never another alias.
- ``remove_alias(alias_iri)``
- ``resolve_alias(alias_iri)`` → canonical IRI or ``None``
- ``get_aliases_for(canonical_iri)`` → list of aliases
- ``is_canonical_iri(iri)`` → ``bool``

Aliases resolve transparently in ``get_graph``, ``get_closure``, ``uri in
env``, and ``env[uri]``.

Runtime configuration
---------------------

Each setting has a getter and a setter. These change an open, writable
environment and persist the change.

.. list-table::
   :header-rows: 1
   :widths: 50 50

   * - Getter
     - Setter
   * - ``is_offline()``
     - ``set_offline(bool)``
   * - ``is_strict()``
     - ``set_strict(bool)``
   * - ``requires_ontology_names()``
     - ``set_require_ontology_names(bool)``
   * - ``remote_cache_ttl_secs()``
     - ``set_remote_cache_ttl_secs(int)``
   * - ``uses_cached_ontologies()``
     - ``set_use_cached_ontologies(bool)``
   * - ``resolution_policy()``
     - ``set_resolution_policy("default" | "latest" | "version")``

Reconfiguration never triggers an implicit scan. Runtime modes apply
immediately; changed discovery paths and filters apply on the next
``update()``.

Container protocols
-------------------

.. code-block:: python

   len(env)              # number of ontologies
   uri in env            # True if uri resolves — canonical name, alias, or source URL
   env[uri]              # shorthand for env.get_graph(uri)
   for name in env: ...  # iterate ontology IRIs

   with OntoEnv.connect("./env") as env:
       ...

``bool(env)`` is always ``True``; use ``env is None`` to test for absence.

Exceptions
----------

.. list-table::
   :header-rows: 1
   :widths: 34 66

   * - Exception
     - Raised when
   * - ``UnresolvedImportError`` (``LookupError``)
     - A *known* unresolved ``owl:imports`` target is passed to
       ``copy_graph``. Covers direct and indirect imports declared by
       catalogued ontologies, and targets attempted while fetching
       dependencies for a transient graph.
   * - ``CatalogRecoveryError`` (``RuntimeError``)
     - Startup found an interrupted-mutation marker. See
       :doc:`../how-to/recover-an-environment`.
   * - ``ExternalStoreChangedError`` (``RuntimeError``)
     - A custom store changed in a way OntoEnv will not reconcile on its own —
       it reports drift it cannot localize to specific graphs, its identity
       does not match the saved catalog, or it changed during a scan. Retry
       with ``sync="full"`` or ``refresh_from_store(full=True)``.
   * - ``StoreCapabilityError`` (``RuntimeError``)
     - An operation needs an optional store method the object does not
       implement.
   * - ``ValueError``
     - Mutating a read-only view, or looking up an IRI that was never
       declared or attempted.

An unknown IRI stays a plain ``ValueError`` precisely so that catching
``UnresolvedImportError`` for expected missing imports does not also swallow
genuine lookup mistakes.
