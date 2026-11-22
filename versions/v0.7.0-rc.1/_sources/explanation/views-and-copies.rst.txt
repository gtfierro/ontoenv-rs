Views and copies
================

The Python graph and dataset query APIs distinguish read-only views from
mutable copies. A long-lived handle from ``OntoEnv.connect(path)`` can keep
using ``env.get_graph``, ``get_closure``, ``get_union`` and ``get_dataset``
directly; each result captures the handle's selected version.

For several queries that must agree, capture one snapshot from that handle.
Its graph, closure and dataset results use the same catalog, aliases,
configuration and RDF:

.. code-block:: python

   # env is the handle kept in application state.
   with env.snapshot() as snapshot:
       graph = snapshot.view_graph(iri)
       closure, names = snapshot.view_closure(iri)
       dataset = snapshot.view_dataset(names)
       editable = snapshot.copy_dataset(names)

   # graph, closure and dataset still own their captured version here.

Snapshot methods use paired ``view_graph`` / ``copy_graph``, ``view_closure`` /
``copy_closure``, ``view_union`` / ``copy_union`` and ``view_dataset`` /
``copy_dataset`` names. Closure and graph union methods return the result plus
the selected canonical IRIs. ``closure_iris(iri, recursion_depth=-1)`` returns
membership alone, without fetching sources or materializing a graph.

``view_dataset(iris=None)`` and ``copy_dataset(iris=None)`` select the whole
captured catalog. An explicit list selects a dataset union; ``[]`` selects no
graphs. Aliases resolve against that snapshot. Unknown IRIs raise ``ValueError``;
cataloged empty graphs are valid contexts. Datasets retain named-graph contexts
and have an empty default graph. The same triple in two contexts remains two
quads; rdflib's ``len(dataset)`` counts unique triples across those contexts.
Graph unions project away the contexts and yield/count each triple once.
SPARQL scans, ``GRAPH`` and ``FROM`` cannot expand a dataset beyond its selection.

Namespace maps merge in selection order, with explicit bindings taking
precedence; a whole-catalog selection uses canonical IRI order. Depth zero
selects only the closure root. Strict mode checks missing imports only when
traversal follows them, so unresolved imports beyond the depth limit do not fail
a query. Cyclic import explanations stop before repeating a node.

``OntoEnv.read_snapshot(path)`` also opens an owned version directly when a
live handle is unnecessary. Snapshot scopes release ownership only; escaped
views and iterators remain usable. There is no read transaction to commit.

The live ``OntoEnv`` convenience reads remain supported:

.. list-table::
   :header-rows: 1
   :widths: 33 33 34

   * - View (read-only)
     - Copy (mutable)
     - Scope
   * - ``get_graph(iri)``
     - ``copy_graph(iri)``
     - one ontology
   * - ``get_closure(iri)``
     - ``copy_closure(iri)``
     - ontology + transitive imports
   * - ``get_union(iris)``
     - ``copy_union(iris, root)``
     - an explicit set of graphs
   * - ``get_dataset()``
     - ``copy_dataset()``
     - the whole environment

Rust currently uses different return types: ``OntoEnv::get_graph`` and
``copy_graph`` both materialize an owned ``oxigraph::model::Graph``. Editing that
value does not change the environment. A managed
``EnvironmentSnapshot::view_graph`` or ``view`` borrows a query view without
materializing the graph. The Python names in the table describe their Python
return values, not a Rust mutability guarantee.

Why the split exists
--------------------

Before 0.6, ``get_closure`` materialized the whole closure into an in-memory
``rdflib.Graph``. For the Brick 1.4.4 closure that is roughly 237,000 triples
built one Python object at a time — and most callers then ran a single query
against it and threw it away.

The view path skips that. It reads term IDs straight out of the memory-mapped
on-disk snapshot and only builds Python objects for the terms you actually
touch. A ``COUNT`` query over that closure runs in about 1.4 ms as a view
versus 110 ms as a materialized graph, because the whole query plan executes
in Rust over integer term IDs and never crosses the FFI boundary per triple.

Copies did not go away — they are just no longer the default for reads.

What a view is
--------------

``get_graph`` returns an ``rdflib.Graph`` backed by OntoEnv's storage.
Mutating it raises ``ValueError``.

``get_closure`` and ``get_union`` return an :class:`ontoenv.ViewGraph`, which
deliberately does **not** subclass ``rdflib.Graph``. It implements the parts
of the interface that make sense read-only:

- ``triples(...)``, ``subjects``/``predicates``/``objects``, iteration
- ``len()``, ``in``, ``bool()``
- ``query(...)`` — SPARQL scoped to the view's graphs
- ``bind`` / ``namespace`` / ``prefix`` / ``namespaces``
- ``serialize(format=...)``

``add``, ``addN``, and ``remove`` raise ``ValueError``. If a library you are
calling requires a real ``rdflib.Graph``, use ``copy_closure`` — that is
exactly the case copies are for.

Same content either way
-----------------------

A ``get_closure`` view and a ``copy_closure`` graph contain the **same triple
set**: imports stripped, ontology declarations collapsed onto the root, SHACL
prefixes consolidated, duplicates removed. The only difference is where the
triples live and whether you can change them.

Snapshot union views and copies contain the same raw triple set. The older
``OntoEnv.get_union`` is also a raw graph projection. Its ``copy_union`` accepts
``rewrite_sh_prefixes=True`` and ``remove_owl_imports=True`` to opt into the
closure transforms — with ``root`` naming the ontology those transforms
collapse onto.

Choosing
--------

**Use a view when** you are querying, counting, iterating, or serializing;
when the graph is large; or when you are in a request handler and need fixed,
read-only graph contents. Python rejects writes to the returned view at runtime.

**Use a copy when** you need to add or remove triples, or when you need an
``rdflib.Graph`` API a view does not implement.

For repeated microsecond-scale ``triples()`` lookups with bound terms, a
plain in-memory rdflib graph is still marginally faster than a view — rdflib's
hash lookup is hard to beat at that scale. If that is your access pattern,
measure before paying for a copy anyway.

Skipping graphs entirely
------------------------

When you only want to stream triples, both wrappers are overhead:

.. code-block:: python

   for s, p, o in env.iter_closure_triples(iri):
       ...

These iterators yield ``(s, p, o)`` tuples of rdflib terms with no ``Graph``
object involved. Note that closure iteration is **not** de-duplicated across
named graphs — unlike a ``ViewGraph``, which is.

.. seealso::

   :doc:`performance` for the measurements behind this page.
