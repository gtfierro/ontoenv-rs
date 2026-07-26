Migrating from 0.5 to 0.6
=========================

OntoEnv 0.6 separates read-only views from mutable copies and introduces an
explicit persistent-environment lifecycle. Existing 0.5 environments migrate
automatically the first time 0.6 opens them.

Choose views or copies
----------------------

``get_graph`` and ``get_closure`` now return read-only views. This avoids
materializing large graphs during ordinary reads. Code that mutates the
returned graph should use the matching copy operation:

.. code-block:: python

   # 0.5: graph = env.get_graph(iri); graph.add(...)
   graph = env.copy_graph(iri)
   graph.add(...)

   # 0.5: closure, names = env.get_closure(iri); closure.add(...)
   closure, names = env.copy_closure(iri)
   closure.add(...)

Use ``get_dataset`` and ``copy_dataset`` for the same distinction at dataset
scope. ``snapshot_as_dataset`` and ``to_rdflib_dataset`` remain as deprecated
compatibility aliases for 0.6.

Use the persistent lifecycle
----------------------------

Use ``connect`` for normal application startup:

.. code-block:: python

   env = OntoEnv.connect("./ontology-env", graph_store=store)

It creates an empty environment on first use, adopts a populated custom store,
and uses the saved catalog for later warm opens. More specific setup flows can
use ``create``, ``open``, or ``adopt``.

The old ``init_from_store=True`` argument is deprecated. Replace first-time
adoption with ``OntoEnv.adopt(path, store)``. Use
``env.refresh_from_store(full=True)`` only when an already-open environment
must deliberately rescan its entire backend.

Refresh the right source
------------------------

``env.update()`` refreshes ontology files and URLs. Pass one source to update
just that source, and use ``force=True`` instead of the deprecated ``all=True``.

``env.refresh_from_store()`` reconciles graphs changed directly in a custom
backend. These operations are intentionally separate.

Handle recovery and missing imports
-----------------------------------

If a process is interrupted between a backend mutation and catalog
publication, normal startup raises ``CatalogRecoveryError``. Recover without
deleting OntoEnv-owned files:

.. code-block:: python

   env = OntoEnv.recover("./ontology-env", graph_store=store)

Recovery scans one stable backend snapshot. If the backend changes or a graph
cannot be read during the scan, recovery fails and leaves its marker in place
so it can be retried safely.

Non-strict import loading remains best-effort. A known unresolved
``owl:imports`` target passed to ``copy_graph`` raises
``UnresolvedImportError``; other lookup, parsing, and backend errors retain
their own error paths, so consumers do not need to match message strings.

Toolchain support
-----------------

Python 3.11 or newer is required. Building the Rust crates from source requires
Rust 1.88 or newer.
