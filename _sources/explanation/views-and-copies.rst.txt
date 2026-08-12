Views and copies
================

Every read method on ``OntoEnv`` comes in two flavours:

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

Unions are the exception, and asymmetrically so. ``get_union`` is always a raw
merge. ``copy_union`` defaults to a raw merge too, but accepts
``rewrite_sh_prefixes=True`` and ``remove_owl_imports=True`` to opt into the
closure transforms — with ``root`` naming the ontology those transforms
collapse onto.

Choosing
--------

**Use a view when** you are querying, counting, iterating, or serializing;
when the graph is large; when you are in a request handler; or when you want a
compile-time guarantee that this code path cannot modify the environment.

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
