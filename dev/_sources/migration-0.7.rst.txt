Upgrading to 0.7
================

This guide covers upgrades from 0.5, 0.6 and earlier unreleased 0.7 builds.
API changes and stored-data migration are separate steps. Existing managed
``.ontoenv/`` environments are **not** migrated automatically by 0.7.

Migrate storage first
---------------------

Close every process using the legacy environment, including readers. Then run
from its root:

.. code-block:: console

   ontoenv migrate

Or call ``OntoEnv.migrate(path)`` in Python or ``OntoEnv::migrate(path)`` in
Rust. Migration validates the stored data and installs ``.ontoenv-snapshots/`` only after a complete version
is ready. It retains the original ``.ontoenv/`` files. Do not run old and new
releases as writers against these two independent copies.

Managed storage keeps **one graph per canonical IRI**. If a legacy catalog has
several registrations for the same canonical IRI, normalize them using the old
release before migrating. Migration rejects that ambiguity without installing
the new directory; it does not silently choose one graph.

Earlier unreleased protocol-2 snapshot stores instead require ``ontoenv upgrade``
or ``OntoEnv.upgrade(path)`` after closing **all older processes**, including
readers. Those binaries do not pin files and cannot run alongside cleanup.

See :doc:`how-to/migrate-storage` for validation, legacy recovery, rollback and
protocol upgrade details. Temporary environments and external ``graph_store=``
backends retain their own storage; managed migration does not convert those
external databases.

Windows deployment contract
---------------------------

Windows 0.7 supports consistent views and recovery after an abrupt process exit.
Power-loss durability is best effort: a power failure can lose an update or leave
an environment that requires rebuilding from ontology sources. A successful
commit or retained previous version does not guarantee recovery of a complete
version after power loss. Existing flushes and integrity checks remain; directory
syncing is unsupported on Windows. See :doc:`explanation/transactional-snapshots`
for the accepted contract.

API replacements
----------------

.. list-table::
   :header-rows: 1
   :widths: 45 55

   * - Removed spelling
     - Replacement
   * - ``OntoEnv(..., create_or_use_cached=True)``
     - ``OntoEnv.connect(path)``
   * - ``OntoEnv(..., init_from_store=True)``
     - ``OntoEnv.adopt(path, store)``; for temporary stores, construct normally
       and call ``refresh_from_store(full=True)``
   * - ``env.update(all=True)`` or ``env.update(True)``
     - ``env.update(force=True)``
   * - ``snapshot_as_dataset(...)`` / ``to_rdflib_dataset(...)``
     - ``get_dataset()`` for a view; ``copy_dataset()`` for editable data
   * - ``temporary_snapshot()`` / Rust ``new_temporary()``
     - ``snapshot()`` for fixed reads; a fresh temporary environment plus
       selected graph copies for independent editing

Live ``OntoEnv.get_*`` convenience reads remain supported. Code upgrading from
0.5 must use ``copy_*`` methods when it needs mutable graphs; see
:doc:`migration-0.6` for the earlier view/copy transition.

Immutable snapshots in 0.7
--------------------------

Use ``env.snapshot()`` or ``OntoEnv.read_snapshot(path)`` for an owned,
immutable version. On these snapshot objects, ``view_graph``, ``view_closure``,
``view_union`` and ``view_dataset`` return read-only views; matching ``copy_*``
methods return mutable copies. The earlier snapshot ``get_graph`` and
``get_dataset`` aliases are removed. Live ``OntoEnv.get_*`` convenience methods
remain available.

The mutable ``temporary_snapshot()`` helper is removed. Create a fresh
``OntoEnv(temporary=True)`` and add selected graph copies when an independent
editable environment is needed.

Rust callers acquire specialized handles with ``SnapshotReader::open`` and
``SnapshotWriter::create/open`` instead of ``OntoEnv::*_snapshot*`` wrappers.
``EnvironmentSnapshot::view_graph`` borrows a query view; ``copy_graph`` on a
snapshot or pending update materializes a graph. ``OntoEnv::adopt`` replaces
``new_with_graph_io_from_existing``; ``refresh_from_store`` replaces
``refresh_from_graph_io``. Fresh temporary environments remain available;
``new_temporary`` as a whole-environment copy helper is removed.

Rust configuration setters and namespace edits now return ``Result``. They
publish before returning on a managed writer, or join an enclosing
``transaction``. ``flush`` and Python ``close`` do not commit pending managed
edits; normal operations already finish their publication. Read-only Rust
configuration overrides remain session-local and reset on refresh.

Rust backend capability
-----------------------

``GraphIO::store()`` becomes ``oxigraph_store() -> Result<&Store>``. Only
memory and caller-supplied Oxigraph stores expose an authoritative raw store.
Managed snapshots, legacy lazy RDF5D stores and Python adapters return a
``StoreCapabilityError``. Prefer graph operations or captured snapshots for
portable reads. The old ``ensure_loaded`` and managed lifecycle hooks are
removed from ``GraphIO``; use the adapter's read methods and ``OntoEnv`` or
snapshot handles respectively.

For copied Python graphs, namespace bindings come from the same captured
selection as the triples. Saved bindings replace destination collisions;
ontology prefixes preserve an explicitly supplied destination's preferences.
New graph copies retain the captured prefixes for queries and serialization.

Python 3.11 or newer and Rust 1.88 or newer remain the supported floors.
