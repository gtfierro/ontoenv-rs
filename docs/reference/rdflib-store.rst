ViewGraph and OntoEnvStore
==========================

The read-only ``rdflib`` surfaces. Both keep SPARQL parsing
(`spargebra <https://crates.io/crates/spargebra>`_) and evaluation
(``spareval``) in Rust, reading triples from the
`rdf5d <https://crates.io/crates/rdf5d>`_ on-disk format via ``mmap`` where
one is available.

For task-oriented guidance see :doc:`../how-to/query-with-sparql`.

.. note::

   ``OntoEnvStore`` is an ``rdflib.store.Store`` that reads *out of* an
   environment. It is unrelated to ``OntoEnv(graph_store=...)``, which is
   storage OntoEnv writes *into* — see :doc:`graph-store`.

``ViewGraph``
-------------

Returned by ``env.get_closure(uri)`` and ``env.get_union(uris)``. A
read-only view over a fixed set of named graphs in the snapshot.

``ViewGraph`` **is** an ``rdflib.Graph`` (``isinstance(view, rdflib.Graph)``),
backed by a read-only store that delegates triple-pattern lookups, ``len``
and SPARQL to the Rust backend scoped to the view's graphs. Everything rdflib
provides on a graph works: ``value()``, ``subject_objects()``, property
paths in ``triples()``, ``qname()``, ``namespaces()``, ``+`` with another
graph, ``serialize()`` (returns ``self`` when given a destination), and
``query(q, initNs=..., initBindings=...)``. The view's prefix bindings apply
to queries. It can be passed to libraries that expect a graph, e.g. as the
data graph of ``pyshacl.validate``.

.. rubric:: Read-only

``add``, ``addN``, ``remove`` and ``+=`` raise ``ValueError``. Use
``copy_closure`` or ``copy_union`` for a mutable merge. Libraries that write
to a graph they are given (pySHACL does this to its *shapes* graph) need a
mutable copy for that argument. ``bind()`` only edits the view's own prefix
table.

.. rubric:: Backing storage

Persistent local environments read closures directly from the rdf5d mmap
snapshot. Temporary environments and those using a custom ``graph_store=``
normalize the closure into a private in-memory read snapshot instead.

.. code-block:: python

   view, names = env.get_closure("https://example.org/site")

   len(view)
   for s, p, o in view: ...
   list(view.subjects(predicate=RDF.type, object=OWL.Ontology))
   view.query("SELECT ?s WHERE { ?s a owl:Ontology }")

``env.get_graph(uri)`` returns a read-only ``rdflib.Graph`` over a single
named graph; unlike ``get_closure`` it applies no closure transform.

``OntoEnvStore``
----------------

An ``rdflib.store.Store`` implementation exposing an environment as normal
``rdflib.Graph`` / ``rdflib.Dataset`` objects. It is registered as the rdflib
plugin name ``"ontoenv"`` once the ``ontoenv`` package is imported.

.. rubric:: Supported

- ``triples``
- ``contexts``
- ``len(graph)``
- namespace binding: ``bind``, ``namespaces``
- SPARQL ``SELECT``, ``ASK``, and graph-producing queries via ``query()``

.. rubric:: Not supported

- ``add``, ``addN``, ``remove`` — raise ``ValueError``. The exposed store is a
  read-only snapshot; mutate the ``OntoEnv`` and take a fresh snapshot.
- SPARQL Update.

.. rubric:: Constructors

.. code-block:: python

   dataset = env.get_dataset()             # usual route

   from rdflib import Graph
   import ontoenv                          # registers the plugin
   graph = Graph(store="ontoenv")

``env.get_dataset()`` binds the environment's known namespaces and keys each
named graph by its ontology IRI. It chooses storage automatically: a zero-copy
rdf5d view over the selected immutable artifact when managed storage exists — unavailable for
temporary environments and those with a custom ``graph_store=`` — and an
in-memory copy otherwise.

A dataset reflects the environment as of the call. After mutating the
environment:

.. code-block:: python

   env.flush()
   env.refresh_dataset(dataset)      # or call env.get_dataset() again

Use ``env.copy_dataset()`` for a mutable in-memory copy.

Namespace bindings can be saved in the environment or edited locally on a
dataset or view. See :doc:`../explanation/namespaces` for precedence rules
and refresh behavior.

Query behavior
--------------

SPARQL executed through ``rdflib`` on either surface is parsed by
``spargebra``, evaluated by ``spareval``, and converted back into rdflib
``Result`` objects.

.. code-block:: python

   graph.query("SELECT ?o WHERE { <urn:s> <urn:p> ?o }")
   dataset.query("SELECT ?g ?s WHERE { GRAPH ?g { ?s ?p ?o } }")

``rdflib`` passes graph-selection hints into the store, so dataset-level
queries such as ``GRAPH ?g`` and union-style dataset queries work without a
second query engine.

Indexing and property-path acceleration are described in
:doc:`../explanation/performance`.

.. rubric:: Runnable example

``python/demo_rdflib_store.py`` in the repository.
