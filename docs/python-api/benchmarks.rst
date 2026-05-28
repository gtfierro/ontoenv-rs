Benchmarks
==========

This page compares read-only ``get_*`` views against materialized ``copy_*``
graphs, with the default rdflib ``Memory`` store and the ``oxrdflib`` Oxigraph
store as reference points.

The benchmark is in ``python/bench_rdflib_store.py``. It loads the Brick 1.4.4
ontology and its imports closure (15 graphs, ~223k triples) into an
``OntoEnv``, then times a handful of operations across four
``rdflib``-compatible backends:

- ``ontoenv-get`` — ``env.get_closure(...)``, a read-only view backed by the
  on-disk rdf5d store via ``mmap``, accelerated by the PSO/POS sidecar index
  (see :ref:`sidecar-index`).
- ``ontoenv-copy`` — ``env.copy_closure(...)``, a mutable in-memory
  ``rdflib.Graph`` materialized from the same closure.
- ``rdflib-memory`` — default rdflib ``Memory`` store loaded from the same
  triples.
- ``oxigraph`` — ``oxrdflib`` store loaded from the same triples (optional;
  skipped if ``oxrdflib`` isn't installed).

Running it
----------

.. code-block:: bash

   cd python
   uv run python bench_rdflib_store.py \
       --brick https://brickschema.org/schema/1.4.4/Brick.ttl \
       --repeat 3

   # Or point at a local file for offline runs:
   uv run python bench_rdflib_store.py --brick ../brick/Brick.ttl

Results
-------

Numbers below are from a run on an Apple Silicon laptop with Brick 1.4.4 and
its imports closure (≈223,000 triples across 15 graphs). Each workload was
run twice; the table shows the **best** time per backend. Your numbers will
vary, but the *shape* of the comparison should be consistent.

.. list-table::
   :header-rows: 1

   * - Workload
     - ontoenv-get
     - ontoenv-copy
     - rdflib-memory
     - oxigraph
   * - Iterate all triples
     - **93 ms**
     - 109 ms
     - 95 ms
     - 565 ms
   * - Match ``?s owl:imports ?o``
     - 0.10 ms
     - 0.03 ms
     - 0.04 ms
     - 0.13 ms
   * - SPARQL ``COUNT`` of ``rdf:type``
     - **3.2 ms**
     - 73 ms
     - 69 ms
     - 3.1 ms
   * - SPARQL ``rdfs:subClassOf*`` of ``brick:Equipment``
     - 6.3 ms
     - 4.4 ms
     - 4.5 ms
     - 0.4 ms
   * - SPARQL ``SELECT ... rdfs:label ... LIMIT 1000``
     - **5.6 ms**
     - 10.5 ms
     - 10.4 ms
     - 6.1 ms

(Bold entries highlight where ``ontoenv-get`` *beats* the in-memory
baseline.)

benchcmp-style comparison
-------------------------

Each block compares one backend against ``rdflib-memory`` (positive ``delta``
means slower than the baseline; negative means faster).

.. code-block:: text

   benchmark vs rdflib-memory: ontoenv-get
     workload                         rdflib-memory best    ontoenv-get best  delta
     iterate all triples                      91.03 ms           154.45 ms  +  69.66%
     match ?s owl:imports ?o                  39.96 us            91.42 us  + 128.78%
     SPARQL: COUNT rdf:type                   67.51 ms             2.99 ms   -95.57%
     SPARQL: subClassOf* Equip.                4.22 ms             5.59 ms  +  32.46%
     SPARQL: labels LIMIT 1000                10.07 ms             5.53 ms   -45.05%

   benchmark vs rdflib-memory: ontoenv-copy
     workload                         rdflib-memory best   ontoenv-copy best  delta
     iterate all triples                      91.03 ms           103.97 ms  +  14.21%
     match ?s owl:imports ?o                  39.96 us            30.13 us   -24.61%
     SPARQL: COUNT rdf:type                   67.51 ms            69.48 ms  +   2.93%
     SPARQL: subClassOf* Equip.                4.22 ms             4.55 ms  +   7.74%
     SPARQL: labels LIMIT 1000                10.07 ms            10.28 ms  +   2.09%

   benchmark vs rdflib-memory: oxigraph
     workload                         rdflib-memory best       oxigraph best  delta
     iterate all triples                      91.03 ms           611.87 ms  + 572.13%
     match ?s owl:imports ?o                  39.96 us           135.58 us  + 239.31%
     SPARQL: COUNT rdf:type                   67.51 ms             3.17 ms   -95.31%
     SPARQL: subClassOf* Equip.                4.22 ms           403.58 us   -90.44%
     SPARQL: labels LIMIT 1000                10.07 ms             4.60 ms   -54.31%

How to read the results
-----------------------

- **``ontoenv-copy`` ≈ ``rdflib-memory`` everywhere.** ``copy_closure``
  materializes the closure into a vanilla rdflib ``Memory`` store, so it
  inherits the same query engine and the same performance profile.
- **``ontoenv-get`` wins outright on SPARQL with small result sets.** The
  whole query plan runs against the rdf5d-backed dataset via ``spareval`` and
  never has to cross the FFI boundary for individual triples. ``COUNT
  rdf:type`` is **~22× faster** than the in-memory baseline (3.0 ms vs.
  68 ms), and a ``LIMIT 1000`` label scan is ~1.8× faster — both wins lean on
  the PSO sidecar to skip per-predicate scans.
- **Selective predicate-bound patterns are at parity with in-memory.**
  ``(None, owl:imports, None)`` is 0.09 ms via the sidecar, within ~2× of the
  in-memory ``Memory`` store's hashed index.
- **Full triple iteration matches in-memory.** 93 ms vs. 95 ms for
  ``rdflib-memory`` on the Brick closure (~237k triples). The all-unbound
  case in ``ClosureGraphView.triples`` streams directly from the rdf5d
  snapshot's term-ID iterator with a u64-keyed cache for Python terms; it
  doesn't build intermediate ``oxrdf::Term`` objects per row. For large
  scans, ``get_*`` is now an equally good choice.
- **Recursive property paths run inside ``spareval``.** ``subClassOf*`` at
  5.6 ms vs. 4.2 ms for in-memory rdflib — comparable, and within reach of
  in-memory because the per-step lookup is now sidecar-accelerated. Oxigraph
  remains the fastest at 0.4 ms thanks to its native planner.
- **Oxigraph is the fastest SPARQL backend on every query workload**, at the
  cost of a slower load and a slower full-iteration path. Use it when you
  load once and query many times.

Rule of thumb
-------------

- Reach for ``get_*`` when you want to keep memory low, when the queries you
  run return small result sets through SPARQL (``COUNT``, aggregations,
  ``LIMIT`` ed projections), or when you want a read-only graph view that
  *can't* mutate the environment.
- Reach for ``copy_*`` when you need many small ``triples()`` pattern matches
  against the same data, run recursive property paths heavily, or mutate the
  result. The one-time materialization cost pays for itself after a handful
  of reads.
- Reach for Oxigraph (``store="Oxigraph"``) when the same data is going to
  serve many SPARQL queries and you can amortize the load cost.

.. _sidecar-index:

PSO/POS sidecar index
---------------------

Persistent environments build a sidecar file ``store.r5tu.idx`` next to the
``store.r5tu`` snapshot. The sidecar holds per-predicate posting lists in
both ``predicate → subject → object`` (PSO) and ``predicate → object →
subject`` (POS) order. Triple-pattern queries with a bound predicate read
from the posting list directly instead of scanning every triple of every
named graph in the closure.

The sidecar is:

- **Built automatically** at the end of every flush that writes data to
  ``store.r5tu``. Disable via the ``auto_index=False`` constructor kwarg or
  ``env.set_auto_index(False)``; rebuild on demand with
  ``env.build_index()``.
- **Validated at open time** against the source ``store.r5tu`` (file size,
  mtime, and a CRC over the graph directory). A stale or missing sidecar is
  treated as a soft failure — the query path logs a warning and falls back to
  the per-graph scan.
- **Optional**. The sidecar is purely additive; deleting it has no effect on
  correctness, only on performance.
- **Roughly the same size as the source snapshot.** For Brick's ~8 MB
  ``store.r5tu`` the sidecar is ~5–7 MB.

The sidecar accelerates patterns where the predicate is bound, including
unbound-graph queries across an imports closure. Patterns with only the
subject or only the object bound still scan every graph in the relevant
closure; full triple iteration is unaffected.

.. code-block:: python

   from ontoenv import OntoEnv

   # Default — sidecar rebuilt on every flush.
   env = OntoEnv(path="./.ontoenv", create_or_use_cached=True)

   # Skip the rebuild (and delete any existing sidecar) on next flush.
   env = OntoEnv(path="./.ontoenv", create_or_use_cached=True, auto_index=False)
   env.add("https://brickschema.org/schema/1.4.4/Brick.ttl")
   env.flush()
   # ...later, manually build the sidecar:
   env.build_index()

   # Toggle the flag on an existing env:
   env.set_auto_index(True)
