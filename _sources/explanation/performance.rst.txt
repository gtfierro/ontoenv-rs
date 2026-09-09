Performance
===========

The 0.6 API separates catalog access from graph access, and views from
materialized graphs. Those choices have measurable consequences: reopening an
environment should depend on catalog size rather than RDF triple count, while
a view trades Python object allocation for a read-only interface. This page
states the measurements, their scope, and the implementation details that
produce them.

Summary of the benchmark
------------------------

- **Warm open took about 10 ms** for both one-graph catalogs tested, containing
  1 and 124,000 triples respectively.
- **For the tested views,** SPARQL took less time than the in-memory rdflib
  baseline, full iteration took a similar amount of time, and individual
  triple lookups took more time.

In ordinary read paths, start with ``get_*``. Use ``copy_*`` where mutation or
the complete ``rdflib.Graph`` API is part of the requirement.

Warm starts
-----------

``python/bench_catalog_warm_start.py`` adopts synthetic one-graph stores
holding 1 and 124,000 triples, then repeatedly reopens each catalog-backed
environment.

Both averaged about 10 ms per open — 9.75 ms and 9.95 ms — on an Apple Silicon
development machine, and neither warm loop ever called ``get_graph``.

.. code-block:: bash

   cd python
   uv run python bench_catalog_warm_start.py

The near-identical result is consistent with the intended boundary: warm-open
cost tracks catalog records, not the size of the graphs they describe. A warm
open reads the catalog and does not call ``get_graph``. This makes
``connect`` suitable for normal process startup; it does not characterize the
cost of later graph operations.

Views versus copies
-------------------

``python/bench_rdflib_store.py`` loads Brick 1.4.4 and its imports closure
(15 graphs, ~237k triples) and times the same operations across four
``rdflib``-compatible backends:

``ontoenv-get``
   ``env.get_closure(...)`` — a read-only zero-copy view over the on-disk
   rdf5d store via ``mmap``, with in-memory permutation indexes.

``ontoenv-copy``
   ``env.copy_closure(...)`` — a mutable in-memory ``rdflib.Graph``
   materialized from the same closure.

``rdflib-memory``
   The default rdflib ``Memory`` store, loaded from the same triples.

``oxigraph``
   The ``oxrdflib`` store, loaded from the same triples. Optional; skipped if
   ``oxrdflib`` is not installed.

.. code-block:: bash

   cd python
   uv run python bench_rdflib_store.py \
       --brick https://brickschema.org/schema/1.4.4/Brick.ttl \
       --repeat 3

   # Or point at a local file for offline runs:
   uv run python bench_rdflib_store.py --brick ../brick/Brick.ttl

Results
-------

The table reports the lower time from two runs on an Apple Silicon laptop.
Absolute times depend on hardware, RDF input, query shape, and dependency
versions. The results apply to this dataset and these workloads.

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
   * - Match ``brick:Equipment ?p ?o``
     - 0.12 ms
     - 0.05 ms
     - 0.05 ms
     - 0.25 ms
   * - Match ``?s ?p owl:Class``
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

Bold marks measurements where ``ontoenv-get`` was lower than the
``rdflib-memory`` baseline.

Measurements cover steady-state reads. Backend construction — view creation,
closure materialization, loading the reference stores — happens before the
timed region, and each workload gets one untimed warm-up. If startup or
one-shot export latency is your actual question, measure that end to end
instead.

Reading the results
-------------------

**``ontoenv-copy`` ≈ ``rdflib-memory`` everywhere.** ``copy_closure``
materializes into a vanilla rdflib ``Memory`` store, so it inherits the same
query engine and the same profile. The timed region excludes the earlier
materialization step.

**The view had lower times for the SPARQL queries with small result sets.** The
whole query plan runs against the rdf5d-backed dataset via ``spareval``,
joining on integer term IDs and never crossing the FFI boundary for individual
triples.
``COUNT rdf:type`` took 1.4 ms through the view and 110 ms through in-memory
rdflib. The ``LIMIT 1000`` label scan took 5.5 ms and 10.5 ms respectively.

**Full iteration matches in-memory.** 131 ms vs 135 ms across ~237k triples.
The all-unbound path streams directly from the snapshot's term-ID iterator
with a u64-keyed cache for Python terms, building no intermediate term objects
per row.

**Microsecond lookups still favour rdflib.** ``owl:imports`` matching is
0.07 ms as a view against 0.04 ms in memory. The in-memory implementation can
answer this pattern with a dictionary lookup. If that is your dominant access
pattern, benchmark it directly.

**Oxigraph had similar times for two of the SPARQL workloads.** In this
benchmark, its load and full-iteration times were higher. If one dataset serves
many queries, measure the complete workload, including loading and
concurrency.

.. _sidecar-index:

Implementation details affecting the results
---------------------------------------------

**Permutation indexes.** Binding an rdf5d snapshot eagerly builds four
posting-list indexes, held in memory for the life of the snapshot:

- ``PSO`` and ``POS`` for a **bound predicate**
- ``SPO`` for a **bound subject** with unbound predicate
- ``OSP`` for a **bound object** with unbound predicate

A triple pattern with any bound term reads the matching posting list instead
of scanning every triple in every graph of the closure. Only the fully
unbound pattern scans directly.

The builds run in parallel threads at bind time — tens of milliseconds per
permutation for the Brick closure — so the cost lands once, up front, rather
than on the first query of each pattern shape. Because an index is built from
the snapshot you just opened, there is nothing to invalidate: no staleness
checks, no sidecar files. Older versions wrote a ``store.r5tu.idx`` sidecar;
it is unused now and removed on the next flush.

The indexes cost roughly a few times the on-disk snapshot's size in RAM, and
are discarded when the snapshot drops. There is no configuration.

.. _pclos-rewriting:

**Property-path closure rewriting.** The query layer also precomputes, in
memory and lazily on first use, the transitive closure of three predicates:
``rdfs:subClassOf``, ``rdfs:subPropertyOf``, and ``owl:sameAs``.

At query time the evaluator intercepts ``?x P+ ?y`` and ``?x P* ?y`` patterns
whose predicate is in that list and substitutes a materialized ``VALUES``
block before handing the query to spareval. Supported shapes: ``P+``, ``P*``,
``^P+``, ``^P*`` for a single IRI ``P``. That is why ``subClassOf*`` runs in
0.57 ms against 4.32 ms for in-memory rdflib.

The rewrite bails out — leaving the path for spareval to evaluate normally —
when the predicate is not in the precomputed list, when the path is a
sequence, alternative, or negated property set rather than a direct ``P+``/``P*``
of one IRI, or when both endpoints of the path are variables.

.. code-block:: python

   from ontoenv import OntoEnv

   # Permutation indexes are built when the rdf5d snapshot is bound.
   env = OntoEnv.connect("./ontology-env")
   env.add("https://brickschema.org/schema/1.4.4/Brick.ttl")
   env.flush()

   # The property-closure table stays lazy: the first supported path query
   # builds it, and later queries reuse it.
   view, _ = env.get_closure("https://brickschema.org/schema/1.4/Brick")

Rules of thumb
--------------

- Use ``get_*`` to keep memory low, for SPARQL returning small result sets
  (``COUNT``, aggregations, ``LIMIT``), and where code must not mutate the
  environment.
- Use ``copy_*`` when mutation or an API exclusive to ``rdflib.Graph`` is
  required.
- For the supported recursive property paths, benchmark ``get_*`` first; the
  closure table specifically accelerates those shapes.
- For an application serving many queries from one loaded dataset, compare
  Oxigraph under representative load and concurrency.
