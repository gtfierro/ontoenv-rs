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
  on-disk rdf5d store via ``mmap``.
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
     - 226 ms
     - 154 ms
     - 136 ms
     - 829 ms
   * - Match ``?s owl:imports ?o``
     - 5.5 ms
     - 0.03 ms
     - 0.05 ms
     - 0.13 ms
   * - SPARQL ``COUNT`` of ``rdf:type``
     - **4.4 ms**
     - 111 ms
     - 111 ms
     - 5.2 ms
   * - SPARQL ``rdfs:subClassOf*`` of ``brick:Equipment``
     - 513 ms
     - 4.5 ms
     - 4.4 ms
     - 0.6 ms
   * - SPARQL ``SELECT ... rdfs:label ... LIMIT 1000``
     - **7.1 ms**
     - 10.5 ms
     - 10.5 ms
     - 4.8 ms

(Bold entries highlight where ``ontoenv-get`` *beats* the in-memory
baseline — it now wins on small-result SPARQL because the whole query plan
stays in Rust against the mmap'd rdf5d snapshot.)

benchcmp-style comparison
-------------------------

Each block compares one backend against ``rdflib-memory`` (positive ``delta``
means slower than the baseline; negative means faster).

.. code-block:: text

   benchmark vs rdflib-memory: ontoenv-get
     workload                         rdflib-memory best    ontoenv-get best  delta
     iterate all triples                     136.45 ms            225.82 ms  +  65.50%
     match ?s owl:imports ?o                  47.92 us              5.54 ms  +11471.97%
     SPARQL: COUNT rdf:type                  110.69 ms              4.37 ms   -96.05%
     SPARQL: subClassOf* Equip.                4.35 ms            513.03 ms  +11692.33%
     SPARQL: labels LIMIT 1000                10.50 ms              7.10 ms   -32.45%

   benchmark vs rdflib-memory: ontoenv-copy
     workload                         rdflib-memory best   ontoenv-copy best  delta
     iterate all triples                     136.45 ms            153.80 ms  +  12.72%
     match ?s owl:imports ?o                  47.92 us             34.46 us   -28.08%
     SPARQL: COUNT rdf:type                  110.69 ms            110.92 ms  +   0.21%
     SPARQL: subClassOf* Equip.                4.35 ms              4.51 ms  +   3.57%
     SPARQL: labels LIMIT 1000                10.50 ms             10.49 ms   -  0.14%

   benchmark vs rdflib-memory: oxigraph
     workload                         rdflib-memory best       oxigraph best  delta
     iterate all triples                     136.45 ms            829.43 ms  + 507.88%
     match ?s owl:imports ?o                  47.92 us            125.42 us  + 161.74%
     SPARQL: COUNT rdf:type                  110.69 ms              5.15 ms   -95.34%
     SPARQL: subClassOf* Equip.                4.35 ms            592.79 us   -86.37%
     SPARQL: labels LIMIT 1000                10.50 ms              4.76 ms   -54.71%

Before/after on ``ontoenv-get``
-------------------------------

The numbers above reflect a series of optimizations to the ``get_*`` read
path. For reference, here is the same set of workloads with ``ontoenv-get``
before vs. after:

.. list-table::
   :header-rows: 1

   * - Workload
     - Before
     - After
     - Speedup
   * - Iterate all triples
     - 5099 ms
     - 226 ms
     - 22.6×
   * - Match ``?s owl:imports ?o``
     - 226 ms
     - 5.5 ms
     - 41×
   * - SPARQL ``COUNT`` of ``rdf:type``
     - 95 ms
     - 4.4 ms
     - 21.6×
   * - SPARQL ``rdfs:subClassOf*``
     - 30432 ms
     - 513 ms
     - 59×
   * - SPARQL ``SELECT ... rdfs:label ... LIMIT 1000``
     - 97 ms
     - 7.1 ms
     - 13.7×

The wins came from four changes in ``python/src/lib.rs`` and one in
``python/ontoenv/rdflib_store.py``:

1. **Filter-before-decode in the SPARQL view.** The
   ``LogicalSparqlDatasetView`` used to decode every term of every scanned
   triple *before* checking it against the bound pattern; now it resolves
   bound terms to rdf5d term IDs once and compares IDs in the inner loop.
   This is what unblocked the SPARQL wins.
2. **Cached rdflib constructor handles.** ``URIRef`` / ``Literal`` /
   ``BNode`` are looked up once per ``triples()`` call and reused, instead
   of doing one ``rdflib.getattr(...)`` per term per triple.
3. **Streaming iterator for ``store.triples()``.** Replaces the upfront
   ``Vec<(Py<PyAny>, ...)>`` with a ``StoreTriplesIter`` that yields one
   row per ``__next__``, so peak memory doesn't scale with the result set
   and the caller can stop early without paying for the tail.
4. **Per-iteration term-ID → Py-object cache.** Brick's closure has ~237k
   triples but only ~30k distinct terms; building each ``URIRef`` /
   ``Literal`` once collapses term construction by roughly 7×.
5. **``ClosureGraphView`` merge moved into Rust.** Previously it iterated
   each named graph in Python and dedup'd with a Python ``set`` of rdflib
   term tuples (slow to hash). Now ``triples_in_graphs``, ``len_in_graphs``,
   and ``contains_in_graphs`` do the merge in one Rust call, dedup'ing at
   the term-ID level.

How to read the results
-----------------------

- **``ontoenv-copy`` ≈ ``rdflib-memory`` everywhere.** Expected:
  ``copy_closure`` materializes the closure into a vanilla rdflib ``Memory``
  store, so it inherits the same query engine and the same performance
  profile.
- **``ontoenv-get`` wins outright on SPARQL with small result sets.** The
  whole query plan runs against the rdf5d-backed dataset via ``spareval``
  and never has to cross the FFI boundary for individual triples.
  ``COUNT rdf:type`` is now **25× faster** than the in-memory baseline
  (4.4 ms vs. 111 ms), and a ``LIMIT 1000`` label scan is ~1.5× faster.
- **``ontoenv-get`` is competitive on full iteration.** Down from 5 s to
  226 ms — still ~1.7× behind the in-memory baseline, because the
  in-memory store doesn't have to construct rdflib term objects at read
  time. For large scans, prefer ``copy_*`` if you'll iterate more than
  once; prefer ``get_*`` if you only need to scan once or want to stop
  early.
- **Small triple-pattern matches still pay for the missing PSO index.**
  ``(None, owl:imports, None)`` is 5.5 ms for 27 hits — 100× faster than
  before, but still well behind the in-memory backends (~50 µs) because
  rdf5d has no per-predicate posting list, so the scan still touches
  every triple in every gid; we just stop *decoding* every triple. Closing
  the last gap requires an index inside the rdf5d format (see the next
  section).
- **Recursive property paths are still slow.** ``subClassOf*`` dropped
  from 30 s to 513 ms but is still 100× slower than in-memory. This is a
  ``spareval`` planner limitation, not an rdf5d/storage problem.
- **Oxigraph is the fastest SPARQL backend on every query workload**, at
  the cost of a slower load and a slower full-iteration path. Use it when
  you load once and query many times.

Rule of thumb
-------------

- Reach for ``get_*`` when you want to keep memory low, when the queries
  you run return small result sets through SPARQL (``COUNT``,
  aggregations, ``LIMIT`` ed projections), or when you want a read-only
  graph view that *can't* mutate the environment. With the recent
  optimizations this is now the default-good choice for most SPARQL.
- Reach for ``copy_*`` when you need many small ``triples()`` pattern
  matches against the same data, run recursive property paths, or mutate
  the result. The one-time materialization cost pays for itself after a
  handful of reads.
- Reach for Oxigraph (``store="Oxigraph"``) when the same data is going
  to serve many SPARQL queries and you can amortize the load cost.

Known performance gaps to close
-------------------------------

The five wins above all stayed *above* the rdf5d storage format. The
remaining gaps need lower-level work:

- **Triple-pattern indexes inside rdf5d (PSO / POS).** rdf5d's only
  indexes are graph-level (``id2gid``, ``gname2gid``, ``pair2gid``); there
  is no per-predicate / per-subject / per-object posting list inside a
  ``gid``. So ``(None, P, None)`` still scans every triple of every gid
  even after the filter-by-ID fix — we just stop *decoding* every triple.
  Closing the last ~100× gap to in-memory's hashed indexes requires real
  PSO/POS posting lists in the rdf5d format. That's a format-level change
  with versioning consequences and is the largest single project on the
  list.
- **``spareval`` property-path planner.** ``subClassOf*`` at 513 ms is no
  longer catastrophic, but it's still 100× behind in-memory rdflib. This
  is the SPARQL evaluator, not storage — different repo, different
  conversation. Until that improves, ``copy_*`` is the right call for
  ``*`` / ``+`` property paths.
