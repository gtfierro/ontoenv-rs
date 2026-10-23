# Compact staging implementation and measurements

Implementation: `5019a76` on the transactional snapshot branch. Parsed imports
now retain deduplicated triple vectors instead of constructing six OxRDF graph
indexes for every staged replacement. Initial import memory and time improve
substantially. The complete API workload still needs more memory than import
alone, and first closure construction is an accepted follow-up.

## Change

The private `StagedGraph` representation has a triple-vector variant and a boxed
indexed-graph variant. Both the normal API adapter and explicit `SnapshotUpdate`
use shared `Arc<StagedGraph>` contents. Parsed inputs move their existing triples
into the vector; original source bytes are dropped after parsing/hash extraction.
Inputs that already supply a `Graph` retain it.

A temporary `HashSet<&Triple>` identifies duplicates while preserving input
order. It is dropped before the owned vector is compacted; strings and terms
are not cloned for deduplication. This explicitly preserves graph set semantics,
which the existing serializer does not enforce for arbitrary duplicate records.
Only the compact owned vector survives staging.

Publication iterates either representation directly. Staged graph reads return
independent materialized graphs, with no persistent staged query-index cache.
Nested checkpoints share the immutable contents. Empty replacements, deletion,
aliases and atomic publication retain their existing behavior. Public API
signatures and the RDF5D format are unchanged. Encoded byte ordering/size can
change because parsed input order differs from indexed graph iteration order.

## Import-only memory

Same corpus and host as `normal-api-memory-profile-2026-10-23.md`: 100 graphs,
19.76 MiB Turtle, 334,059 named-graph quads and 333,008 unique union triples.
Each row below summarizes three fresh sequential processes with default settings.

| Measurement | Before | Compact staging |
| --- | ---: | ---: |
| Native initial import peak RSS | 534–544 MiB | **223–225 MiB** |
| Python initial import peak RSS | 570–572 MiB | **257–260 MiB** |
| Massif useful live heap at publication peak | 456.1 MiB | **200.6 MiB** |
| Massif estimated allocation overhead at that peak | 10.8 MiB | 49.5 MiB |

The native import is under the earlier provisional 256 MiB target for this
fixture. Python's interpreter/package overhead leaves its measured peak slightly
above that target. These results do not establish a general 256 MiB guarantee.

The profiled six-index graph allocation disappears from the ordinary parsed
import path. At the new peak, triple vectors account for about 45.9 MiB and
their parser-allocated terms for about 39.6 MiB, versus the earlier 342.7 MiB
indexed staging allocation. RDF5D serialization remains a significant live allocation. Flat
owned triples also retain more individual small term/string allocations than an
interned graph, reflected in Massif's larger estimated allocation overhead.
Further compaction would need a new measurement rather than subtracting an index
allocation from RSS.

## Complete normal API workload

Three new standalone runs passed all workload assertions: query/count results,
old view/iterator ownership, alias visibility, metadata-only object reuse,
unchanged-update generation stability, refresh/rebind, strict parse rollback and
successful retry. Initial loading took 524–569 ms and the complete workflow
peaked at **439–450 MiB**. Readers, old indexed views and repeated publications
are included in this scope.

An additional comparison alternated three baseline-wheel and three optimized-wheel
processes, all running the same worker with warm OS caches:

| Measurement | Baseline range (median) | Compact range (median) |
| --- | ---: | ---: |
| Initial load and publication | 1,067–1,335 ms (1,311) | 596–662 ms (654) |
| Initial load peak RSS | 561–572 MiB | 258–259 MiB |
| Complete workflow peak RSS | 578–607 MiB | 431–437 MiB |
| First closure construction | 62–76 ms (70) | 74–99 ms (88) |
| First closure query/materialization | 14–28 ms (17) | 15–20 ms (17) |
| One-source update and publication | 576–803 ms (630) | 555–725 ms (623) |
| Reader refresh | 13–28 ms (23) | 24–27 ms (25) |

These are single-host diagnostic ranges, not confidence intervals or latency
SLOs. Alternation reduces some environmental differences but the order was not
randomized. Initial import improved much more than subsequent full-file updates,
which still rewrite the complete RDF5D artifact.

The user explicitly chose to keep this memory-focused change and track first
closure construction separately. Investigate its observed increase before
making a reader-latency improvement claim. Warm query/index behavior, source
ordering/layout changes and measurement variation should be assessed there.

## Validation and reproduction

- Full release Rust workspace: **244 passed**, seven intentional helpers/examples
  ignored, including doctests.
- Release-wheel Python suite: **165 passed**, 21 existing deprecation warnings.
  Tests ran from an isolated copy, with the installed optimized extension;
  concurrency and CLI subprocesses also resolved that installed package.
- New regressions cover duplicate triples, blank-node identity, literal lexical
  forms, independent staged reads, transitions between indexed and parsed
  replacements, and nested rollback through a rename. The prior duplicate-IRI
  migration regression is included.
- Clippy passed for all OntoEnv targets with warnings denied. Formatting and diff
  whitespace checks passed.
- The user-approved dependency test now uses local transitive import fixtures,
  including SHACL prefix ownership assertions. Other URL-fetch tests remain.

The Python diagnostics now reject the repository-local extension and record the
installed extension path. Use `python -I` when invoking their scripts. An
exploratory optimized Python run accidentally loaded the older local extension;
it and interrupted test attempts are excluded from these reported results.

Build the release example and wheel, then run:

```sh
target/release/examples/managed_import_profile NEW_ROOT SOURCE_DIRECTORY --allocator-stats --trim-live
python -I python/profile_managed_import.py NEW_ROOT SOURCE_DIRECTORY
python -I python/bench_managed_ontology.py NEW_ROOT 350
```

Each `NEW_ROOT` must be unused. Import-only workers can use the retained
`/tmp/ontoenv-final-review-run3/sources` corpus. The optimized wheel environment
is `/tmp/ontoenv-flat-staging-venv`; the baseline environment is
`/tmp/ontoenv-final-review-venv`. The baseline library code predates the staging
change; intervening commits changed CI tests and shim packaging only.

Raw data and provenance are in `compact-staging-2026-10-23.json`. The complete
Massif allocation tree is compressed in `compact-staging-massif-2026-10-23.out.gz`.
Local worker directories/logs remain in `/tmp/ontoenv-compact-staging-profile`.
No allocator trim or allocator environment tuning was added to production.
