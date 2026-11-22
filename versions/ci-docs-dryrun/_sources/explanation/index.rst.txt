Explanation
===========

.. raw:: html

   <div class="oe-section-intro">
     Background reading. These pages are about <em>why</em> OntoEnv works the
     way it does — useful when a design decision surprises you, and safe to
     skip until then.
   </div>

:doc:`concepts`
   Ontologies, IRIs, imports, closures, and the environment that ties them
   together. Read this first if the vocabulary is unfamiliar.

:doc:`views-and-copies`
   Why every read method comes in a ``get_*`` and a ``copy_*`` flavour, what a
   closure view actually contains, and how to choose.

:doc:`lifecycle`
   There are five ways to open an environment. What each one refuses to do,
   and why ``connect`` is almost always the right answer.

:doc:`staying-in-sync`
   OntoEnv never reads your files behind your back. What ``update()`` and
   ``refresh_from_store()`` each reconcile, and why they are separate.

:doc:`performance`
   Benchmarks against in-memory rdflib and Oxigraph, plus the indexing that
   explains the results.

.. toctree::
   :hidden:
   :maxdepth: 1

   concepts
   views-and-copies
   lifecycle
   staying-in-sync
   performance
