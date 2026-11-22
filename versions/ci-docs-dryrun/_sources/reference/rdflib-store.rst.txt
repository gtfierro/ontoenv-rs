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
lightweight read-only view over a fixed set of named graphs in the snapshot.

It deliberately does **not** subclass ``rdflib.Graph``. Triple-pattern
lookups, ``len``, ``in``, and ``query()`` are delegated to the Rust backend
scoped to the view's graphs.

.. rubric:: Supported

.. list-table::
   :header-rows: 1
   :widths: 46 54

   * - Member
     - Notes
   * - ``triples(subject=None, predicate=None, obj=None)``
     - Returns an iterator of ``(s, p, o)``.
   * - ``__iter__``, ``__contains__``, ``__len__``, ``__bool__``, ``__repr__``
     - Iteration is de-duplicated across the view's graphs.
   * - ``subjects(...)``, ``predicates(...)``, ``objects(...)``
     - Pattern-restricted and de-duplicated.
   * - ``query(query, init_bindings=None)``
     - SPARQL scoped to the view's graphs.
   * - ``bind(prefix, namespace, override=True)``
     - Namespace binding.
   * - ``namespace(prefix)``, ``prefix(namespace)``, ``namespaces()``
     - Namespace lookup.
   * - ``serialize(format="turtle")``
     - Returns a string.

.. rubric:: Not supported

``add``, ``addN``, and ``remove`` raise ``ValueError``. Use
``copy_closure`` or ``copy_union`` for a mutable merge.

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

Note that ``env.get_graph(uri)`` returns a read-only ``rdflib.Graph``, not a
``ViewGraph``.

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
rdf5d view over ``.ontoenv/store.r5tu`` when one exists — unavailable for
temporary environments and those with a custom ``graph_store=`` — and an
in-memory copy otherwise.

A dataset reflects the environment as of the call. After mutating the
environment:

.. code-block:: python

   env.flush()
   env.refresh_dataset(dataset)      # or call env.get_dataset() again

Use ``env.copy_dataset()`` for a mutable in-memory copy.

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
