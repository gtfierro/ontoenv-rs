Migrating from 0.5 to 0.6
=========================

Existing 0.5 environments migrate automatically the first time 0.6 opens them.
The code changes below are the ones you have to make yourself.

Two changes account for most of it: reads now return read-only views instead
of mutable graphs, and opening an environment has an explicit lifecycle
vocabulary.

At a glance
-----------

.. list-table::
   :header-rows: 1
   :widths: 44 44 12

   * - 0.5
     - 0.6
     - Status
   * - ``env.get_graph(iri)`` then mutate
     - ``env.copy_graph(iri)``
     - breaking
   * - ``env.get_closure(iri)`` then mutate
     - ``env.copy_closure(iri)``
     - breaking
   * - ``OntoEnv(..., create_or_use_cached=True)``
     - ``OntoEnv.connect(path)``
     - deprecated
   * - ``OntoEnv(..., init_from_store=True)``
     - ``OntoEnv.adopt(path, store)``
     - deprecated
   * - ``env.update(all=True)``
     - ``env.update(force=True)``
     - deprecated
   * - ``env.snapshot_as_dataset(...)``
     - ``env.get_dataset()`` / ``env.copy_dataset()``
     - deprecated
   * - ``env.to_rdflib_dataset(...)``
     - ``env.get_dataset()`` / ``env.copy_dataset()``
     - deprecated
   * - ``refresh_dataset_from_env(dataset, env)``
     - ``env.refresh_dataset(dataset)``
     - moved

Views and copies
----------------

``get_graph`` and ``get_closure`` no longer materialize their result, which
avoids building a large in-memory graph for reads that only query it. Code
that mutates the returned graph must switch to the matching copy method:

.. code-block:: python

   # 0.5: graph = env.get_graph(iri); graph.add(...)
   graph = env.copy_graph(iri)
   graph.add(...)

   # 0.5: closure, names = env.get_closure(iri); closure.add(...)
   closure, names = env.copy_closure(iri)
   closure.add(...)

``get_closure`` and ``get_union`` return an :class:`ontoenv.ViewGraph`, which
does **not** subclass ``rdflib.Graph``. If you pass the result to something
that requires a real ``rdflib.Graph``, use ``copy_closure``.

The same distinction applies at dataset scope: ``get_dataset`` for a view,
``copy_dataset`` for a mutable copy.

.. note::

   ``get_union`` is a **raw** merge and ``copy_union`` now defaults to one
   too — ``rewrite_sh_prefixes`` and ``remove_owl_imports`` both default to
   ``False`` there. ``copy_closure`` still defaults both to ``True``. If you
   relied on ``copy_union`` applying the closure transforms, pass them
   explicitly.

Background: :doc:`explanation/views-and-copies`.

Lifecycle
---------

Use ``connect`` for normal startup:

.. code-block:: python

   env = OntoEnv.connect("./ontology-env", graph_store=store)

It creates an empty environment on first use, adopts a populated custom store,
and warm-opens from the saved catalog afterwards. ``create``, ``open``, and
``adopt`` express narrower requirements.

Replace ``init_from_store=True`` with ``OntoEnv.adopt(path, store)``. Use
``env.refresh_from_store(full=True)`` only when an already-open environment
must deliberately rescan its whole backend.

``create_or_use_cached=True`` now emits ``DeprecationWarning``. The shim
remains through 0.6.x; removal is planned for 0.7.

Background: :doc:`explanation/lifecycle`.

Refreshing
----------

``env.update()`` refreshes ontology files and URLs. It now takes an optional
source to update just that one, and ``force=True`` replaces the deprecated
``all=True``.

``env.refresh_from_store()`` reconciles graphs changed directly in a custom
backend. The two are deliberately separate — see
:doc:`explanation/staying-in-sync`.

Recovery
--------

If a process is interrupted between a backend mutation and catalog
publication, startup raises ``CatalogRecoveryError``. Rebuild without deleting
OntoEnv-owned files:

.. code-block:: python

   env = OntoEnv.recover("./ontology-env", graph_store=store)

For the built-in store, ``ontoenv recover`` does the same from the command
line. Recovery scans one stable backend snapshot; if the backend changes or a
graph cannot be read during the scan, it fails and leaves its marker so it can
be retried safely.

See :doc:`how-to/recover-an-environment`.

Configuration on reopen
-----------------------

``OntoEnv.open`` and ``OntoEnv.connect`` preserve every persisted setting whose
option you omit. Explicit values — including ``False``, ``"default"``, and
empty lists — override. Writable connections save overrides; read-only ones
apply them for that session only.

This covers strict, offline, name validation, cache settings, resolution
policy, remote cache TTL, search directories, and every filter list. Overrides
do not trigger an implicit scan; discovery settings take effect on the next
``update()``.

Missing imports
---------------

A known unresolved ``owl:imports`` target passed to ``copy_graph`` now raises
:class:`ontoenv.UnresolvedImportError`, a ``LookupError``. This includes
direct and indirect imports declared by catalogued ontologies and targets
attempted while fetching dependencies for a transient caller graph.

An IRI that was never declared or attempted anywhere remains a plain
``ValueError``, so you no longer need to catch ``ValueError`` broadly for
expected missing imports.

Non-strict import loading remains best-effort. Completed
``import_dependencies(..., fetch_missing=True)`` and
``get_dependencies(..., fetch_missing=True)`` calls commit their partial
results and leave no recovery marker even when imports remain unavailable. A
marker now means only an interrupted or failed commit.

Rust API
--------

``GraphIO::union_graph`` returns ``(Dataset, Vec<FailedImport>)`` and is always
best-effort: per-id errors are recorded and the offending id skipped, but the
rest of the union is still assembled. Previously failures were dropped
silently.

``OntoEnv::get_union_graph`` consumes that list — in strict mode any failure
becomes an error; in non-strict mode the partial union is returned with
``UnionGraph.failed_imports`` populated.

Toolchain
---------

Python 3.11 or newer. Building the Rust crates from source requires Rust 1.88
or newer, matching rdf5d's edition 2024 and the resolved dependency floor.
