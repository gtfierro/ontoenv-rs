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
  on-disk rdf5d store via ``mmap``, accelerated by in-memory permutation
  indexes built on demand (see :ref:`sidecar-index`).
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
     - **131 ms**
     - 154 ms
     - 135 ms
     - 784 ms
   * - Match ``?s owl:imports ?o``
     - 0.07 ms
     - 0.04 ms
     - 0.04 ms
     - 0.13 ms
   * - Match ``brick:Equipment ?p ?o`` (subject-only)
     - 0.12 ms
     - 0.05 ms
     - 0.05 ms
     - 0.25 ms
   * - Match ``?s ?p owl:Class`` (object-only)
     - 1.53 ms
     - 1.48 ms
     - 1.33 ms
     - 5.02 ms
   * - SPARQL ``COUNT`` of ``rdf:type``
     - **1.41 ms**
     - 114 ms
     - 110 ms
     - 5.30 ms
   * - SPARQL ``rdfs:subClassOf*`` of ``brick:Equipment``
     - **0.57 ms**
     - 4.46 ms
     - 4.32 ms
     - 0.47 ms
   * - SPARQL ``SELECT ... rdfs:label ... LIMIT 1000``
     - **5.48 ms**
     - 10.99 ms
     - 10.52 ms
     - 5.50 ms

(Bold entries highlight where ``ontoenv-get`` *beats* the in-memory
baseline.)

benchcmp-style comparison
-------------------------

Each block compares one backend against a baseline (positive ``delta`` means
slower than the baseline; negative means faster). The default baseline is
``rdflib-memory``; an extra ``ontoenv-get vs oxigraph`` block is also emitted
because Oxigraph is the most direct alternative for read-only ``get_*``
workloads.

.. code-block:: text

   ontoenv-get vs rdflib-memory
     workload                         rdflib-memory best ontoenv-get best  delta
     iterate all triples                     134.54 ms        131.72 ms    -2.10%
     match ?s owl:imports ?o                  39.46 us         73.75 us  +  86.91%
     match Equipment ?p ?o                    53.67 us        121.96 us  + 127.25%
     match ?s ?p owl:Class                     1.33 ms          1.53 ms  +  15.20%
     SPARQL: COUNT rdf:type                  109.82 ms          1.41 ms   -98.71%
     SPARQL: subClassOf* Equip.                4.32 ms        570.96 us   -86.79%
     SPARQL: labels LIMIT 1000                10.52 ms          5.48 ms   -47.90%

   ontoenv-copy vs rdflib-memory
     workload                         rdflib-memory best ontoenv-copy best  delta
     iterate all triples                     134.54 ms        154.49 ms  +  14.82%
     match ?s owl:imports ?o                  39.46 us         36.46 us    -7.60%
     match Equipment ?p ?o                    53.67 us         54.96 us  +   2.41%
     match ?s ?p owl:Class                     1.33 ms          1.48 ms  +  11.21%
     SPARQL: COUNT rdf:type                  109.82 ms        113.63 ms  +   3.47%
     SPARQL: subClassOf* Equip.                4.32 ms          4.46 ms  +   3.12%
     SPARQL: labels LIMIT 1000                10.52 ms         10.99 ms  +   4.39%

   oxigraph vs rdflib-memory
     workload                         rdflib-memory best    oxigraph best  delta
     iterate all triples                     134.54 ms        783.96 ms  + 482.68%
     match ?s owl:imports ?o                  39.46 us        127.33 us  + 222.71%
     match Equipment ?p ?o                    53.67 us        251.08 us  + 367.85%
     match ?s ?p owl:Class                     1.33 ms          5.02 ms  + 277.85%
     SPARQL: COUNT rdf:type                  109.82 ms          5.30 ms   -95.18%
     SPARQL: subClassOf* Equip.                4.32 ms        471.88 us   -89.08%
     SPARQL: labels LIMIT 1000                10.52 ms          5.50 ms   -47.77%

   ontoenv-get vs oxigraph
     workload                            oxigraph best ontoenv-get best  delta
     iterate all triples                     783.96 ms        131.72 ms   -83.20%
     match ?s owl:imports ?o                 127.33 us         73.75 us   -42.08%
     match Equipment ?p ?o                   251.08 us        121.96 us   -51.43%
     match ?s ?p owl:Class                     5.02 ms          1.53 ms   -69.51%
     SPARQL: COUNT rdf:type                    5.30 ms          1.41 ms   -73.36%
     SPARQL: subClassOf* Equip.              471.88 us        570.96 us  +  21.00%
     SPARQL: labels LIMIT 1000                 5.50 ms          5.48 ms    -0.24%

How to read the results
-----------------------

- **``ontoenv-copy`` ≈ ``rdflib-memory`` everywhere.** ``copy_closure``
  materializes the closure into a vanilla rdflib ``Memory`` store, so it
  inherits the same query engine and the same performance profile.
- **``ontoenv-get`` wins outright on SPARQL with small result sets.** The
  whole query plan runs against the rdf5d-backed dataset via ``spareval``,
  joining on integer term ids and never crossing the FFI boundary for
  individual triples. ``COUNT rdf:type`` is **~78× faster** than the
  in-memory baseline (1.4 ms vs. 110 ms), and a ``LIMIT 1000`` label scan is
  ~1.9× faster — both wins lean on the PSO index to skip per-predicate
  scans.
- **Triple patterns with any bound term are served from an index.** A
  bound predicate uses PSO/POS, a bound subject uses SPO, a bound object uses
  OSP — so ``brick:Equipment ?p ?o`` (0.12 ms) and ``?s ?p owl:Class``
  (1.53 ms) skip the per-graph scan that the unindexed fallback would do
  (those same patterns were ~5–7 ms before the SPO/OSP sections existed).
  rdflib's pure in-memory hash still edges ahead on the microsecond-scale
  matches (e.g. ``owl:imports`` at 0.07 ms vs. 0.04 ms), but ``ontoenv-get``
  now beats Oxigraph on every triple-pattern shape.
- **Full triple iteration matches in-memory.** 131 ms vs. 135 ms for
  ``rdflib-memory`` on the Brick closure (~237k triples). The all-unbound
  case in ``ViewGraph.triples`` streams directly from the rdf5d
  snapshot's term-ID iterator with a u64-keyed cache for Python terms; it
  doesn't build intermediate ``oxrdf::Term`` objects per row. For large
  scans, ``get_*`` is now an equally good choice.
- **Recursive property paths short-circuit through the closure index.**
  ``subClassOf*`` at 0.57 ms vs. 4.32 ms for in-memory rdflib (~7.6×
  faster) and at parity with Oxigraph's 0.47 ms. See
  :ref:`pclos-rewriting` below.
- **Oxigraph remains a strong SPARQL backend**, but ``ontoenv-get`` now
  matches or beats it on every query workload here, at the cost of a slower
  load and a slower full-iteration path on Oxigraph's side. Reach for
  Oxigraph when you load once and query many times with the full SPARQL
  surface.

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

In-memory query indexes
-----------------------

The first query that needs one triggers the construction of a permutation
index, held in memory for the life of the snapshot. Nothing is written to
disk and there is no configuration. Indexes are built in four orders:

- ``predicate → subject → object`` (PSO) and ``predicate → object →
  subject`` (POS) for patterns with a **bound predicate**;
- ``subject → predicate → object`` (SPO) for patterns with a **bound
  subject** and unbound predicate (``(s, ?, ?)`` / ``(s, ?, o)``);
- ``object → subject → predicate`` (OSP) for patterns with a **bound
  object** and unbound predicate (``(?, ?, o)``).

A triple-pattern query reads the matching posting list directly instead of
scanning every triple of every named graph in the closure.

The indexes are:

- **Built lazily and per-permutation.** Each order is constructed on first
  use, so a workload only pays for the permutations it actually queries. The
  build walks the snapshot once and sorts in memory — on the order of tens of
  milliseconds per permutation for the Brick closure (~237k triples), then
  free on every subsequent query.
- **Always fresh.** Because an index is built from the snapshot you just
  opened, there is nothing to invalidate — no staleness checks, no sidecar
  files to keep in sync. (Older versions wrote a ``store.r5tu.idx`` sidecar;
  it is no longer used and is removed on the next flush.)
- **Held in RAM**, roughly a few times the on-disk snapshot's size for the
  full set of permutations. They are discarded when the snapshot is dropped.

The indexes accelerate every triple pattern with at least one bound term —
bound predicate (PSO/POS), bound subject (SPO), or bound object (OSP) —
including unbound-graph queries across an imports closure. Only the
fully-unbound pattern (``(?, ?, ?)``, i.e. full triple iteration) scans the
closure directly.

.. _pclos-rewriting:

Property-path closure rewriting
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

The query layer also precomputes (in memory, on first use) the transitive
closure of a fixed list of predicates (``rdfs:subClassOf``,
``rdfs:subPropertyOf``, ``owl:sameAs``). At query time, the SPARQL evaluator
intercepts ``?x P+ ?y`` / ``?x P* ?y`` patterns whose predicate is in the
list and substitutes a materialized ``VALUES`` block before handing the
query to spareval. Supported path shapes: ``P+``, ``P*``, ``^P+``, ``^P*``
where ``P`` is a single IRI.

Bail-out cases (the path is left intact and spareval evaluates it
itself, exactly as before):

- Predicate is not in the precomputed closure list.
- Path is a sequence, alternative, negated-property-set, or otherwise
  not a direct ``P+``/``P*`` of a single IRI.
- Both endpoints of the path are variables.

.. code-block:: python

   from ontoenv import OntoEnv

   # Indexes are built in memory on demand — no setup required.
   env = OntoEnv(path="./.ontoenv", create_or_use_cached=True)
   env.add("https://brickschema.org/schema/1.4.4/Brick.ttl")
   env.flush()

   # The first subClassOf* query builds the closure index; later ones reuse it.
   g = env.get_closure("https://brickschema.org/schema/1.4.4/Brick")
