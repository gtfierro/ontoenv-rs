Recover an interrupted environment
==================================

.. note::

   This interrupted-mutation procedure applies to legacy and caller-provided
   stores. The default 0.7 managed backend opens the complete version selected
   by ``CURRENT`` after process exit, without a recovery writer. Invalid selected
   state raises an error; it never scans for a fallback. See
   :doc:`migrate-storage` for explicit legacy migration.


The symptom
-----------

A command or a ``connect`` call fails with a recovery error:

.. code-block:: console

   $ ontoenv status
   Error: OntoEnv recovery required: interrupted mutation marker at
   ./.ontoenv/catalog.pending; run `ontoenv recover` or call OntoEnv::recover
   to rebuild the catalog

In Python this surfaces as :class:`ontoenv.CatalogRecoveryError`.

This means a process stopped between changing the graph store and publishing
the updated index. The store may contain changes that the index does not
describe. OntoEnv refuses to use that index until recovery scans the stored
graphs and replaces it.

The fix
-------

.. code-block:: console

   $ ontoenv recover
   Recovered catalog at ./.ontoenv with 12 ontology records.

.. code-block:: python

   from ontoenv import OntoEnv, CatalogRecoveryError

   try:
       env = OntoEnv.connect("./ontology-env")
   except CatalogRecoveryError:
       env = OntoEnv.recover("./ontology-env")

With a custom graph store, pass it:

.. code-block:: python

   env = OntoEnv.recover("./ontology-env", graph_store=store)

Recovery scans every stored graph and publishes a replacement index. It is
much slower than a normal open, so it is not something OntoEnv does for you
automatically.

``ontoenv recover`` uses the same environment discovery as every other
command: it walks up from the current directory and honours ``ONTOENV_DIR``.

.. warning::

   Do not delete ``.ontoenv/catalog.pending`` by hand. The marker is what
   tells OntoEnv the index is untrustworthy; removing it makes a possibly
   incomplete index look valid. ``recover`` removes it only after the
   replacement index is successfully published.

If recovery fails
-----------------

Recovery requires a stable, fully readable snapshot of the backend. It aborts
and leaves the marker in place if a graph cannot be read or the backend
changes mid-scan — so the operation is always safe to retry.

Stop anything else writing to the environment, then run it again.

When this is *not* the problem
------------------------------

A missing ``owl:imports`` target does **not** leave a recovery marker. In
non-strict mode, ``import_dependencies(..., fetch_missing=True)`` and
``get_dependencies(..., fetch_missing=True)`` are best-effort: they skip what
they cannot reach and commit the partial result cleanly.

So a recovery marker always means an interrupted or failed commit, never
merely an unresolved import. For missing imports, see
:doc:`diagnose-imports`.

Recovery is unavailable for temporary environments (``--temporary`` /
``OntoEnv(temporary=True)``), which have nothing persisted to recover from.

Start over instead
------------------

If you would rather rebuild from your source files than recover:

.. code-block:: console

   $ ontoenv init ./ontologies --overwrite

For legacy stores, ``reset`` asks for confirmation and deletes ``.ontoenv/``.
For managed 0.7 storage, it publishes an empty catalog while retaining old
artifacts and views. To repopulate an existing managed environment, use
``ontoenv init ./ontologies --overwrite``; this publishes the requested source
scan as a replacement version. Source files are not deleted. Reset does not
reclaim managed disk space.
