Graph store protocol
====================

The interface a ``graph_store=`` object must satisfy for OntoEnv to route all
graph reads and writes through it. For task-oriented guidance see
:doc:`../how-to/use-a-custom-graph-store`.

Graphs are always passed and returned as ``rdflib.Graph`` instances.

Required methods
----------------

.. list-table::
   :header-rows: 1
   :widths: 50 50

   * - Signature
     - Contract
   * - ``add_graph(iri: str, graph: Graph, overwrite: bool = False) -> None``
     - Store *graph* under *iri*. With ``overwrite=False``, leave an existing
       graph untouched.
   * - ``get_graph(iri: str) -> Graph``
     - Return the graph for *iri*, for read-only access. Backs every ``get_*``
       method.
   * - ``remove_graph(iri: str) -> None``
     - Delete the graph for *iri*.
   * - ``graph_ids() -> list[str]``
     - Return every currently stored IRI.

Optional methods
----------------

.. list-table::
   :header-rows: 1
   :widths: 40 60

   * - Signature
     - Effect when implemented
   * - ``copy_graph(iri: str) -> Graph``
     - Used by ``copy_graph``, ``copy_closure``, ``copy_union``, and
       ``copy_dataset`` to obtain a detached mutable copy. Falls back to
       ``get_graph`` when absent. Implement it when your store distinguishes a
       live view from a snapshot.
   * - ``size() -> dict[str, int]``
     - Returns ``{"num_graphs": ..., "num_triples": ...}`` for diagnostics.
   * - ``store_state() -> dict[str, str]``
     - Returns opaque ``id`` and ``revision`` strings, enabling O(1) identity
       and external-drift detection.
   * - ``graph_revisions() -> dict[str, str]``
     - Returns an opaque revision per graph, enabling incremental refresh of
       only what changed.

Which optional methods you implement determines what OntoEnv can do about
changes made outside it:

.. list-table::
   :header-rows: 1
   :widths: 40 60

   * - Implemented
     - Behavior on ``connect(sync="auto")``
   * - ``graph_revisions``
     - Reads only added and changed graphs; removes deleted ones.
   * - ``store_state`` only
     - Detects drift, cannot localize it; raises and asks for ``sync="full"``.
   * - Neither
     - Trusts the saved catalog until you request a refresh.

Constraints
-----------

- ``graph_store=`` cannot be combined with ``recreate=True`` or with the
  deprecated ``create_or_use_cached=True``.
- Recovery for a custom store must go through
  ``OntoEnv.recover(path, graph_store=store)``; the ``ontoenv recover`` CLI
  command only handles the built-in persistent store.
- A temporary environment with a custom store has no saved catalog, so a
  pre-populated store needs an explicit ``refresh_from_store(full=True)``.

Minimal implementation
----------------------

.. code-block:: python

   from rdflib import Graph
   from ontoenv import OntoEnv


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

       def size(self) -> dict[str, int]:
           return {
               "num_graphs": len(self.graphs),
               "num_triples": sum(len(g) for g in self.graphs.values()),
           }


   store = DictGraphStore()
   env = OntoEnv(graph_store=store, temporary=True)
   env.add("./ontologies/site.ttl")
   print(store.graph_ids())

Synchronization API
-------------------

``env.refresh_from_store(graphs=None, full=False)`` returns a ``SyncReport``
with ``added``, ``changed``, and ``removed`` attributes.

- No arguments — incremental, driven by ``graph_revisions()``.
- ``graphs=[...]`` — exactly those backend graph IDs; never expanded.
- ``full=True`` — rescan everything. Cannot be combined with ``graphs``.

Store synchronization never reads ontology source files or URLs and never
fetches imports. Use ``env.update()`` for that. See
:doc:`../explanation/staying-in-sync`.
