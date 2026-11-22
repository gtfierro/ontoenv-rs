Staying in sync
===============

.. _refreshing-ontology-sources:

OntoEnv has a saved view of the world, and the world moves. Files get edited,
remote ontologies get republished, and — if you supplied your own graph store —
something else may write into that store directly.

There are two reconciliation operations, and they solve genuinely different
problems.

.. list-table::
   :header-rows: 1
   :widths: 26 37 37

   * -
     - ``env.update()``
     - ``env.refresh_from_store()``
   * - Reconciles
     - Ontology **sources**: files and URLs
     - Graphs changed **directly in a custom store**
   * - Reads
     - The filesystem and the network
     - Your ``graph_store`` object
   * - Follows ``owl:imports``
     - Yes
     - No
   * - Relevant when
     - Always
     - Only with ``graph_store=``

Both exist because they answer different questions. "Has ``site.ttl`` changed
on disk, and if so what does it import now?" is not the same question as "did
another process write into my database while I wasn't looking?" Collapsing
them into one call would mean every restart either checks the network or
scans your whole backend.

Refreshing sources
------------------

.. code-block:: python

   env.update()                                  # changed files + expired remotes
   env.update(force=True)                        # every known source, regardless
   env.update("https://example.org/site.ttl")    # one source
   env.update("https://example.org/site.ttl", force=True)

``update()`` checks the configured search directories for new, changed, and
removed files, and re-fetches remote ontologies whose cached copies have
outlived the TTL. It follows ``owl:imports`` throughout, so dependencies stay
current along with the ontologies that pulled them in.

With a location argument it updates that one source and replaces its stored
graph, again following imports.

Whether ``update`` touches the network at all depends on the environment's
``offline`` and cache settings. See :doc:`../how-to/work-offline`.

Why connect doesn't do this for you
-----------------------------------

Because reading files and fetching URLs is expensive, and startup is exactly
when you least want to pay for it.

A service that restarts under load would otherwise re-parse every ontology on
every restart. Keeping ``update()`` explicit means the cost is scheduled by
you — at deploy time, on a timer, from an admin endpoint — rather than
imposed by the library.

Reconciling a custom store
--------------------------

This section only matters if you passed ``graph_store=``.

Changes made **through** ``env`` update your store and OntoEnv's catalog
together; nothing extra is needed. Changes made **directly to the store** are
different, because OntoEnv did not see them happen.

How much OntoEnv can do about that depends entirely on what your store can
tell it:

**The store reports per-graph revisions** (``graph_revisions()``). OntoEnv
rereads only the graphs that actually changed. This is the good case, and the
reason that optional method is worth implementing.

**The store reports that it changed, but not which graphs**
(``store_state()`` only). OntoEnv raises rather than guessing, and asks you to
request ``sync="full"`` explicitly.

**The store reports nothing.** OntoEnv trusts its saved catalog until you ask
for a refresh.

That middle case is a deliberate design choice. ``sync="auto"`` will never
silently turn a normal restart into a full scan of your database — if the only
correct action is expensive, you get to decide when to pay for it.

.. code-block:: python

   report = env.refresh_from_store()                    # incremental
   report = env.refresh_from_store(graphs=[iri, ...])   # exactly these
   report = env.refresh_from_store(full=True)           # forget and rebuild

   print(report.added, report.changed, report.removed)

Targeted refreshes are exact
----------------------------

``graphs=`` is an exact set of backend graph IDs. OntoEnv does not expand it —
if you name a root, you get the root, not its imports. To refresh a root along
with the closure already recorded for it, compose the two calls:

.. code-block:: python

   report = env.refresh_from_store(graphs=env.list_closure(root))

That works when a known closure was rewritten together. But if the external
edit introduced a *new* imported graph, that graph is not in the old closure
yet, so a targeted refresh will not see it. An incremental refresh finds it
when the store reports per-graph changes; otherwise ``full=True`` is the
answer.

A targeted refresh also says nothing about graphs it did not look at. The
report lists those as still pending, and a later connection will still detect
that the store and the catalog have drifted.

Synchronization at connect time
-------------------------------

``sync=`` controls the same machinery during ``connect``:

``"auto"`` (default)
   Create or index on first use, reuse the saved catalog on restart,
   incorporate external changes when the store can identify them. Never reads
   ontology sources.

``"full"``
   Reread every graph in the store. For known out-of-band changes the store
   cannot identify.

``"catalog"``
   Use the saved catalog without reading graph contents at all. For controlled
   deployments where another part of the system guarantees the saved view is
   correct.

None of these fetch ontology sources. After connecting, call ``update()`` when
files and URLs should be refreshed too.

When commits are interrupted
----------------------------

A mutation writes graphs, then publishes the updated catalog. If a process
dies between those steps, a ``catalog.pending`` marker remains and the next
open raises ``CatalogRecoveryError`` rather than trusting a catalog that might
not describe every graph.

Normal completed mutations remove the marker as part of their commit —
including best-effort ones. In non-strict mode,
``import_dependencies(..., fetch_missing=True)`` and
``get_dependencies(..., fetch_missing=True)`` may skip imports they cannot
reach, but the partial result they *did* commit is a complete, valid commit.
So a recovery marker always means an interrupted or failed write, never merely
a missing import.

See :doc:`../how-to/recover-an-environment`.
