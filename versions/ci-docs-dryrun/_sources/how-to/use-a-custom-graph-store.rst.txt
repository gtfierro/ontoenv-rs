Use your own graph storage
==========================

If you already manage graph storage — a database, a triplestore, an in-memory
dict — you can have OntoEnv read and write through it instead of its built-in
storage. OntoEnv keeps doing import resolution, naming, and closure lookup;
your object holds the triples.

.. note::

   This is not the same thing as the ``rdflib`` store integration. A
   ``graph_store=`` object is *storage OntoEnv writes into*; ``OntoEnvStore``
   is an ``rdflib.store.Store`` that *reads out of* an environment. See
   :doc:`query-with-sparql` for the latter.

Write a store
-------------

Implement four methods. Graphs are always passed as ``rdflib.Graph``
instances.

.. code-block:: python

   from rdflib import Graph


   class DictGraphStore:
       def __init__(self) -> None:
           self.graphs: dict[str, Graph] = {}

       def add_graph(self, iri: str, graph: Graph, overwrite: bool = False) -> None:
           if not overwrite and iri in self.graphs:
               return
           self.graphs[iri] = graph

       def get_graph(self, iri: str) -> Graph:
           return self.graphs[iri]

       def remove_graph(self, iri: str) -> None:
           del self.graphs[iri]

       def graph_ids(self) -> list[str]:
           return list(self.graphs.keys())

Register it
-----------

.. code-block:: python

   store = DictGraphStore()
   env = OntoEnv.connect("./ontology-env", graph_store=store)

   env.add("./ontologies/site.ttl")
   print(store.graph_ids())
   # ['https://example.org/site', 'https://example.org/sensors']

Changes you make through ``env`` update your store and OntoEnv's index
together. No separate synchronization call is needed.

For a scratch environment with no saved index:

.. code-block:: python

   env = OntoEnv(graph_store=store, temporary=True)

.. warning::

   ``graph_store=`` cannot be combined with ``recreate=True`` or the
   deprecated ``create_or_use_cached=True``.

Connect to a store that already has graphs
------------------------------------------

On the first connection to a populated store, OntoEnv reads each graph once to
learn its ontology IRI, imports, aliases, and namespaces. It does not fetch
network imports while doing so. Later connections reuse the saved index.

.. code-block:: python

   env = OntoEnv.connect("./ontology-env", graph_store=store)

To make that first-time indexing an explicit step rather than something
``connect`` decides:

.. code-block:: python

   env = OntoEnv.adopt("./ontology-env", graph_store=store)

For a *temporary* environment with a pre-populated store, there is no saved
index to fall back on, so ask for the scan directly:

.. code-block:: python

   env = OntoEnv(graph_store=store, temporary=True)
   env.refresh_from_store(full=True)

Pick up changes made behind OntoEnv's back
------------------------------------------

When something else writes into your store, OntoEnv did not see it happen. How
it finds out depends on what your store can report.

Add these optional methods to make incremental refresh possible:

.. code-block:: python

   def store_state(self) -> dict[str, str]:
       """Opaque `id` and `revision` — O(1) drift detection."""
       return {"id": self.store_id, "revision": str(self.revision)}

   def graph_revisions(self) -> dict[str, str]:
       """Opaque revision per graph — enables incremental refresh."""
       return {iri: self.revisions[iri] for iri in self.graphs}

With ``graph_revisions``, a plain refresh reads only what changed:

.. code-block:: python

   report = env.refresh_from_store()
   print(report.added, report.changed, report.removed)

To check specific graphs and nothing else:

.. code-block:: python

   report = env.refresh_from_store(graphs=["https://example.org/site"])

   # A root plus the imports closure already recorded for it:
   root = "https://example.org/site"
   report = env.refresh_from_store(graphs=env.list_closure(root))

``graphs`` is an exact set of backend graph IDs — OntoEnv does not expand it.
A targeted refresh deliberately says nothing about other changed graphs; the
report lists those as still pending.

To forget the saved index and rebuild it from everything currently in the
store:

.. code-block:: python

   report = env.refresh_from_store(full=True)

Pass ``graphs=`` or ``full=True``, not both.

Choose synchronization at connect time
--------------------------------------

.. list-table::
   :header-rows: 1
   :widths: 20 80

   * - ``sync=``
     - Behavior
   * - ``"auto"`` (default)
     - Create or index on first use; reuse the saved index on restart;
       incorporate external changes when the store can identify them.
   * - ``"full"``
     - Reread every graph. Use after out-of-band changes your store cannot
       identify.
   * - ``"catalog"``
     - Use the saved index without reading graph contents at all.

If the store reports that it changed but cannot say *which* graphs changed,
``connect`` raises rather than silently scanning everything. Make the cost
explicit:

.. code-block:: python

   env = OntoEnv.connect("./ontology-env", graph_store=store, sync="full")

That refusal is the point: a normal restart never quietly turns into a full
scan of your database.

Store synchronization never touches ontology *sources*. It does not read files
or fetch URLs — use ``env.update()`` for that.
:doc:`../explanation/staying-in-sync` explains why these are separate.

Optional extras
---------------

.. code-block:: python

   def copy_graph(self, iri: str) -> Graph:
       """Detached mutable copy, used by copy_graph/copy_closure/copy_union/copy_dataset.
       Falls back to get_graph() when absent."""

   def size(self) -> dict[str, int]:
       """{"num_graphs": ..., "num_triples": ...} for diagnostics."""

Implement ``copy_graph`` when your store distinguishes a live view from a
detached snapshot — a database cursor versus an in-memory copy, for instance.

.. seealso::

   :doc:`../reference/graph-store` for the complete protocol.
