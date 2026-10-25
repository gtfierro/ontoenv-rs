# Consolidation handoff

## Current task

The user approved the consolidation plan, added a dataset consistency note,
and then explicitly requested reading this handoff and beginning implementation.
The first Phase 1 slice is implemented on `refactor/graph-io-capabilities`:
fallible raw Oxigraph capability, scratch-store removal, contract documentation,
API inventory and regression coverage. The Phase 2 query slice now adds shared
catalog traversal, immutable snapshot queries and scoped dataset unions. The
backing comparison is complete and committed in `a601eca`. The managed-state
rewrite and API pruning are implemented and locally validated. Do
not treat context reset as permission to discard the working tree.

## Latest workflow instruction

After Phase 1 commit `6121b3a`, the user instructed: no draft PR, do everything
on the current branch, and make one big push at the end. Continue the remaining
approved consolidation locally on `refactor/graph-io-capabilities`; do not create
more branches/PRs or push intermediate phases. Phase 1 was already pushed and
draft PR #38 was created before that steering arrived; #38 is now closed. PR #36
and its storage branch remain untouched. Local commits are suitable checkpoints.

The user clarified that memory limits are not a
current priority. The 512 MB estimate was rough and must not become a gating
requirement. Resume the original consolidation with a simple, working,
performant implementation. Defer the separately proposed query-index memory
optimization and any configurable memory limits. Keep the current mmap/pin
approach and Windows owned fallback. The Linux comparison is complete; see
`snapshot-backing-profile-2026-10-25.md` and its raw/summary artifacts.

## Workspace and baseline

- Work in `/tmp/ontoenv-transactional-snapshots`.
- Branch: `refactor/graph-io-capabilities`, based on
  `feature/transactional-snapshots`. Inspect current HEAD and its PR state.
- Baseline HEAD: `ca264072a8cb7715138d440b48ff493062abb55e`.
- PR #36: <https://github.com/gtfierro/ontoenv-rs/pull/36>.
- Last checked, all 22 CI checks passed at that exact baseline, including full
  Linux/macOS/Windows jobs, snapshot suites, Rust 1.88, Python 3.11/3.14,
  migration and docs. Recheck if HEAD or the remote state changes.
- The main checkout `/home/gabe/src/ontoenv-rs` is on a different branch
  (`gtf-namespaces`) and has unrelated untracked files. Leave it alone.
- Preserve the existing untracked `docs/reviews/final-review-2026-10-23.md`.
  It records an older review and is not the current CI state.
- Preserve the approved consolidation plan and this handoff, including the
  user's verbatim dataset note in the plan.

## Plan and accepted decisions

Read `docs/reviews/consolidation-plan-2026-10-25.md` in this worktree.

- Breaking API cleanup is explicitly permitted in unreleased 0.7. Compatibility
  wrappers are not required merely to preserve the unreleased API.
- Approximately 14 MB encoded is representative. Approximately 100 MB encoded
  is the high end, with fewer than ten reader processes. Use nine readers as
  the upper-bound experiment point.
- Local storage, infrequent writes, one writer, independent reader processes.
- Keep atomic publication of coherent graph/catalog/alias/configuration state.
- Owned views/iterators remain usable after parent-handle close, publication
  by another process, and writer restart. Read scopes release ownership only.
- Keep one graph per canonical IRI and full-file graph rewrites for now.
- Windows is a deployment target.
- Do not add another database, query engine, multiple writers, background GC,
  general history APIs or per-graph disk COW during consolidation.
- Dataset APIs must use the same view/copy model as graphs. Allow dataset
  results for graph unions, with explicit named-graph and duplicate semantics.
  Exact names/return-type choices should be settled in the Phase 1 proposal.
- Memory measurements are advisory. Keep the existing backing/pinning model;
  do not chase the rough 512 MB estimate or start the separate index-memory
  project. Prioritize a simple working implementation and ordinary performance.
- Preserve existing flushing. Windows power-loss guarantees remain a separate
  release decision. Retaining an older file does not establish recovery.
- No merge, release or tag has been requested.

## Phase 1 scope and continuation

The public contract inventory is in `api-contract-inventory-2026-10-25.md`.
The raw-store boundary is implemented. Inspect current git/PR state before
continuing or repeating work. Proceed autonomously within the approved scope;
check in for material product decisions, not routine implementation choices.

1. Read the plan and relevant repository instructions. Only `rdf5d/AGENTS.md`
   was found previously; recheck applicable instructions for changed files.
2. Inventory Rust/Python/CLI lifecycle, query, view/copy, mutation and maintenance
   operations. Identify retained, removable and adapter-specific contracts.
3. Implemented: `GraphIO::store() -> &Store` is replaced with fallible
   `oxigraph_store()`. Memory/caller-supplied Oxigraph backends expose the actual
   store; managed, lazy legacy and Python backends return `StoreCapabilityError`.
   Both scratch stores and the obsolete trait-level `ensure_loaded` hook are
   removed. Managed flush is explicitly independent of the raw capability.
4. Evaluate each backend honestly: a lazily loaded or shadow Oxigraph store is
   not automatically an authoritative capability. Migrate internal callers to
   graph/query methods or explicit supported access. Do not replace the fake
   store with a panic or invent a generic capability registry.
5. Define graph/dataset view-versus-copy contracts and clarify canonical-IRI
   conflict/discovery policies. Update contradictory documentation as those
   contracts are settled.
6. Keep this first implementation slice separate from snapshot-query extraction,
   owned-byte policy changes, and the larger update-state rewrite.

Relevant locations:

- `lib/src/io.rs`: `GraphIO::oxigraph_store()` and its fallible default operations;
  legacy private cache reads, memory/external capability implementations.
- `lib/src/snapshot_io.rs`: no scratch store; explicit no-op backend flush.
- `lib/src/errors.rs`: existing `StoreCapabilityError`.
- `lib/src/api.rs`: public IO access, lifecycle paths, metadata checkpoints and
  batching; these later require consolidation into one committed/pending state.
- `python/src/lib.rs`: PythonGraphIO routes graph operations to the Python store;
  no scratch store/raw capability. Public lifecycle/query methods and dataset cache.
- `python/src/snapshots.rs`: immutable Python snapshot currently supports graphs,
  datasets and metadata, but closure/union querying still lives on OntoEnv.
- `python/src/ontoenv.pyi`, `python/ontoenv/rdflib_store.py`: public dataset and
  view/copy contracts.
- `docs/explanation/concepts.rst`: resolution-policy text must agree with the
  one-canonical-IRI catalog invariant.

Search current raw access with `rg -n 'oxigraph_store' lib python cli`.
The old `.store()`/`fn store()` search should find no GraphIO accessor or calls;
distinguish the unrelated atomic `.store(...)` call in progress reporting.

## Later phases

2. Complete shared snapshot-based querying, including dataset view/copy and
   union results; preserve transforms, aliases, namespaces and iterator lifetime.
3. Compare owned encoded bytes versus mmap using the same codec and normal APIs.
   Measure aggregate memory and retained versions at approximately 14/100 MB
   and 1/4/9 readers. No permanent user-facing backing toggle by default.
4. Consolidate committed/pending state and necessary rollback checkpoints.
5. Remove obsolete API paths and isolate external-store/legacy migration logic.

Do not combine a storage-policy switch and transaction-state rewrite in one
change. Removing pins would require protecting older mmap readers through an
explicit protocol boundary, even though the public owned-view contract stays.

## Validation and practical notes

- Reusable Cargo target: `/tmp/ontoenv-snapshot-review-target`.
- Run meaningful focused regressions for real behavior, then relevant clippy,
  formatting and broader checks. Do not add tests that only mirror code.
- Phase 1 needs managed capability refusal, authoritative supported-store access,
  accurate normal reads/counts and affected external/custom-store coverage.
- Local results for the implemented slice: 256 default-feature release Rust
  workspace tests/doctests passed (eight ignored); all four capability tests
  passed with and without RocksDB; 97 targeted Python tests passed against the
  rebuilt installed wheel (21 existing warnings). Workspace all-target/all-feature
  release clippy, formatting/diff checks and Sphinx HTML passed. Check the new
  branch's CI rather than attributing baseline PR #36 results to this change.
- All-feature local builds can compile RocksDB for a long time; start with
  appropriate default-feature checks and expand when the change warrants it.
- Python source-tree `_native.abi3.so` can shadow a rebuilt installed wheel.
  Verify the imported extension. Use an isolated copied test directory for
  installed-wheel checks; `test_ontoenv_api.py` expects a sibling `../brick`.
- Existing URL-fetch Python tests need network access. A prior failing QUDT
  dependency-closure test was deliberately replaced with local fixtures; the
  dedicated URL tests remain.
- GH CLI work requires the gh-cli skill. Installed `gh pr edit` previously
  failed on deprecated projectCards; REST `gh api --method PATCH` works.
- Check in with the user on meaningful decisions. Do not spawn sub-agents
  unless the user or applicable instructions explicitly request them.

## Phase 2 query slice

- `lib/src/catalog_queries.rs` contains the single closure traversal and importer
  explanation logic. Snapshots cache its immutable derived index; live queries
  use it for committed reads and derive from the candidate during batches.
  Runtime strict overrides still apply to live queries. Namespace merging is
  shared, with deterministic canonical ordering for whole-catalog queries.
- `EnvironmentSnapshot` now exposes closure membership, importers/paths,
  missing-import explanations, namespaces and dataset/graph-union copies.
  Existing RDF5D graph/view primitives remain the Rust query-view surface; final
  public naming cleanup belongs to Phase 5.
- Python snapshots add `view_graph`/`copy_graph`, `view_closure`/`copy_closure`,
  `view_union`/`copy_union`, `view_dataset`/`copy_dataset`, `closure_iris`, import
  paths and explanations. Closures retain existing transform behavior: selected
  canonical import terms are removed; explicit alias terms are preserved.
- Dataset scopes resolve aliases and validate every selected graph, preserve
  canonical contexts (including empty graphs), and distinguish all from empty.
  Native scans, counts, contexts, GRAPH, FROM and path evaluation enforce scope.
- Graph union iteration/count/query results now deduplicate shared triples.
  Dataset quad iteration returns rdflib Graph contexts; dataset length counts
  unique triples, while quad iteration retains duplicates across contexts.
  RDF5D's synthetic union default graph now deduplicates across named graphs.
- Strict closures check only imports actually traversed within the depth limit;
  cyclic importer paths stop before revisiting a node. These fix previous
  disagreements between indexed and fallback queries.
- Local checks: 262 release Rust workspace tests/doctests passed (eight ignored);
  101 targeted Python tests passed against the rebuilt installed wheel (34
  rdflib/deprecation warnings). All-target/all-feature release clippy,
  formatting/diff and Sphinx HTML passed. Logs:
  `/tmp/ontoenv-consolidation-phase2-{workspace,python,clippy,docs}.log`.
  Do not attribute Phase 1 or baseline CI to this code.

Next: consolidate committed/pending state and remove redundant public/adapter
paths. Preserve the existing backing, pins and durability barriers. Do not chase
the advisory memory estimate or add a memory-limit API. The profile found
whole-store query-index costs, but the user explicitly deprioritized that
separate optimization in favor of finishing the simple working implementation.

## Phase 4 working slice

- Private `Backend` in `lib/src/backend.rs` distinguishes managed storage from
  adapters. `OntoEnv` no longer owns a second environment/config copy. Managed
  idle reads borrow the selected `EnvironmentSnapshot` directly.
- `SnapshotCandidate` owns environment, configuration and staged graph changes.
  Both normal source operations and `SnapshotUpdate` use it and the same writer
  publication path. Managed nesting checkpoints this complete candidate once;
  the outer checkpoint remains until publication succeeds.
- `SnapshotGraphIO` no longer needs a mutex, separate graph-only checkpoints or
  per-pending-graph metadata copies. Mutation uses ordinary exclusive Rust
  borrowing. Session-only reader config overrides are explicitly separate.
- Removed unused dependency graph/index caches from `OntoEnv` and its rollback
  state. Source import loading/validation remains; read indexes derive from the
  captured catalog. Adapter-only metadata rollback is `AdapterCheckpoint`.
- Managed lifecycle hooks were removed from public `GraphIO`; the concrete
  backend handles snapshot, refresh, publication and prune. `save_to_directory`
  now takes `&mut self`; retained Rust/Python/CLI callers have been migrated.
- New regressions cover publication failure restoring preexisting settings,
  graphs and catalog together; reader overrides followed by refresh; and
  unchanged configuration not creating a spurious pending update.
- A normal ~14 MB Python workload passed counts, queries, retained views and
  iterators, alias publication, parse rollback and retry. The tracked artifact is
  `consolidation-state-workload-2026-10-25.json` (working tree based on `a601eca`).
  Initial import was ~961 ms, one-source full-file publication ~1,028 ms and
  reader refresh ~32 ms in this warm-cache diagnostic; these are not SLOs.
- Local validation after the no-op configuration fix: 265 release Rust
  tests/doctests passed (eight ignored), 101 targeted rebuilt-wheel Python tests
  passed (34 rdflib/deprecation warnings), all-target/all-feature release clippy
  and formatting/diff checks passed. Logs:
  `/tmp/ontoenv-consolidation-state-{workspace,python,clippy}.log`.

Then continue Phase 5 cleanup: remove obsolete public aliases/constructor flags,
keep adapters and maintenance distinct, and document the primary view/copy and
mutation workflow. Preserve useful temporary and custom-store behavior. No
memory-limit API, backing switch or durability change is part of this work.


## Phase 5 completion

- Removed deprecated constructor flags, dataset aliases, positional update
  booleans/`all=`, and the mutable whole-environment temporary-copy helper.
  Python immutable snapshots expose paired `view_*`/`copy_*` methods; live
  `OntoEnv.get_*` convenience reads remain. Rust snapshots use `view_graph` and
  `copy_graph`; specialized handles are acquired directly. Removed duplicate
  adoption/refresh wrappers and the empty snapshot feature flag.
- Configuration and namespace edits now publish before return, or join an
  enclosing managed transaction. Rust setters return `Result`. Failed publication
  restores the candidate and runtime flags; no-op settings do not publish.
  Read-only Rust overrides remain session-local. Managed flush/close do not
  create an extra commit boundary. Adapters retain explicit reconciliation.
- Fixed an independent query correctness issue: cached closure rewriting is
  restricted to indexed predicates. The expanded regression failed before the
  guard when an unrelated RDFS predicate enabled rewriting. Custom paths now
  fall back to the general evaluator.
- Updated callers, stubs, examples and migration guidance. Explicit rdflib
  dataset rebinding remains for integration; existing iterators retain their
  original selection. No backing, durability, memory-budget or reader-protocol
  change was made.
- Final local checks: 266 release Rust tests/doctests passed (eight ignored),
  168 installed-wheel Python tests passed (115 focused + 53 source/API tests),
  all-target/all-feature release clippy with denied warnings, formatting/diff
  checks and strict Sphinx HTML passed. Source tests/docs needed network access.
  Logs: `/tmp/ontoenv-consolidation-api-{rust,python,python-sources,clippy,docs}.log`.
- Normal ~14 MB workload passed counts, graph/closure/dataset queries, retained
  views/iterators, alias publication, parse rollback, retry and close. Artifact:
  `consolidation-api-workload-2026-10-25.json`. Initial import ~1,005 ms,
  one-source full-file publication ~1,031 ms, refresh ~26 ms (warm diagnostics).
- Finish with ONE push, then verify Linux/macOS/Windows CI at that exact commit.
  No PR, merge, tag or release is authorized. Windows power-loss durability
  remains a release decision; memory limits and separate index-memory work stay
  deferred. Preserve the older untracked final-review file.


## Final integration follow-up

The planned final batch was pushed as `eea32aa`. CI passed Linux, MSRV, both
Python versions, upgrade and docs; both macOS/Windows job variants exposed one
test fixture using its pre-canonicalization creation config for `publish_config`.
The strict-closure fixture now clones the effective selected config. The original
failure reproduces under a Linux symlinked TMPDIR; afterward the full snapshot
suite passes there (23 passed, two helpers ignored). The root-change validation
is unchanged. A corrective commit/push follows; verify that new commit's full
matrix rather than treating the initial partial results as final. No additional
implementation work is planned if that matrix passes.
