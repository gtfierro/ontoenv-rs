Python Graph Store Interface
============================

OntoEnv can write graphs into a caller-provided Python store via the
``graph_store`` argument. This is useful when integrating OntoEnv into
applications that already manage graph storage.

Interface
---------

Provide an object that implements the following methods:

.. code-block:: python

   class GraphStore:
       def add_graph(self, iri: str, graph: Graph, overwrite: bool = False) -> None: ...
       def get_graph(self, iri: str) -> Graph: ...
       def remove_graph(self, iri: str) -> None: ...
       def graph_ids(self) -> list[str]: ...
       def size(self) -> dict[str, int]: ...  # optional: {"num_graphs": ..., "num_triples": ...}

Notes
-----

- ``graph_store`` cannot be combined with ``recreate`` or ``create_or_use_cached``.
- Graphs are passed as ``rdflib.Graph`` instances.

Example
-------

Here is a minimal in-memory store and how to register it:

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
