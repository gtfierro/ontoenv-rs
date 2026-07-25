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
           — retrieve a graph by IRI for read-only access (views, <code>get_*</code> methods).</li>
       <li><code>remove_graph(iri: str) → None</code>
           — delete a graph from the store.</li>
       <li><code>graph_ids() → list[str]</code>
           — return all currently stored IRIs.</li>
     </ul>
     <h3>Optional</h3>
     <ul>
       <li><code>copy_graph(iri: str) → Graph</code>
           — return a mutable copy of the graph for use by <code>copy_graph</code>,
           <code>copy_closure</code>, <code>copy_union</code>, and
           <code>copy_dataset</code>. When absent, those methods fall back to
           <code>get_graph</code>. Implement this when your store distinguishes
           between a live view and a detached mutable copy (e.g. a database
           cursor vs. an in-memory snapshot).</li>
       <li><code>size() → dict[str, int]</code>
           — return <code>{"num_graphs": …, "num_triples": …}</code> for diagnostic use.</li>
       <li><code>store_state() → dict[str, str]</code>
           — return opaque <code>id</code> and <code>revision</code> strings for O(1)
           identity and external-drift detection.</li>
       <li><code>graph_revisions() → dict[str, str]</code>
           — return an opaque revision for each graph, enabling incremental refresh.</li>
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

For a pre-populated temporary store, request the scan explicitly:

.. code-block:: python

   env = OntoEnv(graph_store=store, temporary=True)
   report = env.refresh_from_store(full=True)
   print(report.added)

Persistent stores should use the explicit lifecycle methods:

.. code-block:: python

   with OntoEnv.create("./new-environment", graph_store=store) as env:
       pass

   with OntoEnv.open("./existing-environment", graph_store=store) as env:
       pass

   with OntoEnv.adopt("./adopted-environment", store) as env:
       pass

``adopt`` and ``refresh_from_store(full=True)`` are the only operations that
deliberately scan every stored graph. A normal open trusts ``catalog.r5tu`` and
never calls ``get_graph``. If ``graph_revisions`` is unavailable, call
``refresh_from_store(graphs=[...])`` or request ``full=True``.
