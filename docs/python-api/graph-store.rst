Graph Store Interface
=====================

.. raw:: html

   <div class="oe-section-intro">
     OntoEnv can route all graph reads and writes through a caller-provided Python object.
     This is useful when you already manage graph storage — an in-memory dict, a database,
     or a custom triplestore — and want OntoEnv to slot in without touching the filesystem.
   </div>

Protocol
--------

Pass a ``graph_store=`` object to ``OntoEnv()``. It must implement the following methods:

.. raw:: html

   <div class="oe-protocol-box">
     <h3>Required</h3>
     <ul>
       <li><code>add_graph(iri: str, graph: Graph, overwrite: bool = False) → None</code>
           — store a graph under the given IRI.</li>
       <li><code>get_graph(iri: str) → Graph</code>
           — retrieve a previously stored graph by IRI.</li>
       <li><code>remove_graph(iri: str) → None</code>
           — delete a graph from the store.</li>
       <li><code>graph_ids() → list[str]</code>
           — return all currently stored IRIs.</li>
     </ul>
     <h3>Optional</h3>
     <ul>
       <li><code>size() → dict[str, int]</code>
           — return <code>{"num_graphs": …, "num_triples": …}</code> for diagnostic use.</li>
     </ul>
   </div>

.. raw:: html

   <div class="oe-tip">
     <span class="oe-tip-icon">&#x26A0;&#xFE0F;</span>
     <p>
       <strong>Constraint:</strong> <code>graph_store</code> cannot be combined with
       <code>recreate=True</code> or <code>create_or_use_cached</code>. Graphs are always
       passed as <code>rdflib.Graph</code> instances.
     </p>
   </div>

Example
-------

A minimal in-memory store and how to register it:

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

   env.add("./ontologies/my.ttl")
   print(store.graph_ids())   # ['https://example.com/myOntology']
