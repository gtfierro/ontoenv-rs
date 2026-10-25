# Consolidation review

The approved five-phase consolidation is implemented. The result keeps the
owned-view and independent-reader guarantees while reducing state duplication
and redundant public entry points. Memory limits and a separate query-index
memory optimization remain deferred by the user's explicit steering.

## Resulting model

- A managed handle reads one selected immutable `EnvironmentSnapshot`.
- A writer has one candidate containing catalog, configuration and changed
  graphs. Nested checkpoints share staged graph buffers and restore that
  candidate as a unit, including through publication failure and unwinding.
- Normal graph, alias and configuration edits publish before return. Explicit
  Rust transactions group operations. Flush/close do not add a managed commit
  boundary. Read-only overrides are session-local.
- Catalog traversal is shared by live candidates and immutable snapshots.
  Derived query indexes belong to their captured snapshot/view. Refresh replaces
  a selection; existing views and iterators retain their backing and file pin.
- Private concrete backend dispatch isolates adapters and their weaker
  reconciliation/recovery contract. Unsupported raw Oxigraph access returns a
  typed error instead of an empty scratch store.

The remaining writer lock, immutable files/manifests, atomic selection and
cross-process pins serve the accepted concurrency requirements. No database,
new query engine, history API, background GC or per-graph disk COW was added.
Full-file graph rewrites and one graph per canonical IRI remain explicit.
Migration rejects duplicate canonical IRIs before installation and preserves
legacy files. Cleanup retains current/previous publications and live pinned
versions; the offline boundary protects older readers that cannot pin.

## API changes

Immutable Python selections use `view_graph/closure/union/dataset` and matching
`copy_*` methods. Explicit dataset selection retains named contexts, including
empty graphs; graph projections deduplicate triples. Live `OntoEnv.get_*`
convenience reads remain. Rust snapshots use `view_graph` and `copy_graph`.

Removed deprecated constructor flags, dataset aliases, positional update
booleans/`all=`, mutable whole-environment temporary-copy helpers, duplicate Rust
snapshot acquisition/adoption/refresh wrappers, and the empty Cargo feature.
Fresh temporary environments and custom stores remain supported. The migration
guide maps removed names to replacements. Explicit rdflib dataset rebinding is
retained because its integration and in-flight iterator behavior are covered.

An additional correctness regression exposed custom property paths returning
empty results when another indexed predicate enabled closure rewriting. The
optimizer now handles only predicates covered by its closure index and lets
the general evaluator handle the others.

## Evidence and limits

- 266 release Rust workspace tests/doctests pass; eight existing ignores.
- 168 installed-wheel Python tests pass on Python 3.14: 115 focused lifecycle,
  snapshot, dataset, adapter, CLI and concurrency tests plus 53 source/API tests.
- All-target/all-feature release clippy passes with warnings denied. Formatting,
  diff checks and strict Sphinx HTML pass. The docs build uses the rebuilt native
  extension; URL tests and reference inventories use network access.
- The representative normal Python API workload passes counts, query results,
  generations, retained views/iterators, alias publication, parse rollback,
  retry and close. See `consolidation-api-workload-2026-10-25.json`: roughly
  14 MB encoded, initial import 1,005 ms, a one-source full-file publication
  1,031 ms and reader refresh 26 ms. These warm diagnostics are not SLOs.
- The earlier 14/100 MB backing comparison and nine-process retention checks
  remain recorded in `snapshot-backing-profile-2026-10-25.md`. They inform the
  retained mmap/pin choice; they impose no memory ceiling.

Integration CI must be checked at the final pushed commit; prior branch/baseline
results do not validate this revision. Windows directory-sync support and its
power-loss durability contract remain a release decision. This refactor
preserves barriers and does not strengthen that guarantee. No merge or release
is included in this work.


## Integration follow-up

The first final push (`eea32aa`) passed Linux, Rust 1.88, Python 3.11/3.14,
upgrade and docs. Both macOS and Windows jobs exposed the same strict-closure
test fixture error: it reused the configuration supplied before creation had
canonicalized the storage root. The fixture now edits the selected snapshot's
configuration, preserving the root guard. A symlinked temporary-root reproduction
failed before the correction and the full snapshot suite passed afterward
(23 passed, two child-process helpers ignored). No publication/storage behavior
changed. A small corrective push is needed despite the planned single batch;
its complete matrix must be verified independently.
