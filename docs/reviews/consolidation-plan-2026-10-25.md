# OntoEnv consolidation plan

Status: approved plan, incorporating the user's compatibility, workload and
dataset decisions. Baseline: `ca26407`, after the owned-view work in PR #36.
The first Phase 1 slice is implemented on `refactor/graph-io-capabilities`; see
`api-contract-inventory-2026-10-25.md`. The Phase 2 query slice implements shared
catalog traversal and immutable graph/closure/union/dataset queries. The backing
comparison is recorded in `snapshot-backing-profile-2026-10-25.md`.
Managed-state consolidation and public API pruning are implemented. Final local
validation passes 266 Rust tests/doctests, 168 Python tests, lint, docs and the
representative normal-API workload. Remote integration CI follows the final push.

## Objective

Keep OntoEnv focused on discovering ontologies, resolving their identities and
imports, and querying/exporting a consistent dependency closure. Reduce the
number of lifecycle rules and persistence models that maintainers and ordinary
API users must understand.

Preserve these requirements:

- Local storage, infrequent writes, one writer and independent reader processes.
- One immutable selection containing coherent graphs, catalog, aliases and
  configuration.
- Views and iterators remain usable after their originating handle closes,
  another process publishes, or the writer restarts.
- Source updates and reader refresh are distinct operations. Opening a reader
  does not fetch or update ontology sources.
- A failed source operation cannot expose a partially updated environment.
  Uncertain publication outcomes require reconciliation before retrying.
- Read scopes release ownership; there is no read transaction to commit.
- Windows is a deployment target, alongside Linux and macOS.

Continue to accept complete RDF5D rewrites for graph-changing publications while
writes are rare. Keep one graph per canonical IRI. Do not add multiple writers,
per-graph disk COW, general version-history APIs, background GC, a new database,
or a new query engine during this consolidation.

## Decisions to record

| Decision | Draft recommendation | Status |
| --- | --- | --- |
| Public API compatibility | Allow breaking API cleanup in unreleased 0.7. Remove misleading contracts and redundant entry points; provide migration guidance instead of mandatory compatibility wrappers. | Confirmed by user. |
| Workload envelope | Approximately 14 MB encoded is representative; approximately 100 MB is the high end, with fewer than ten simultaneous reader processes. | Confirmed by user; use nine readers for the upper-bound experiment. |
| Aggregate reader memory | Record measured costs; defer memory limits and budget-driven optimization. | User clarified that the rough 512 MB estimate is advisory and must not gate consolidation or release. Prioritize a simple, working, performant implementation. |
| Snapshot backing | Keep the existing backing and lifetime-pin approach. | Linux comparison complete: mmap materially reduces multiprocess duplication; switching to owned bytes does not simplify query-index costs. No product backing toggle or protocol change. |
| Durability | Preserve existing flushing during refactoring. Specify process-crash behavior separately from power-loss recovery and latest-update persistence. | Confirmed by user: Windows 0.7 accepts process-crash consistency with best-effort power-loss durability. Power failure may require rebuilding from sources; no complete previous version is guaranteed. Existing barriers remain. |

The user explicitly deprioritized the earlier rough 512 MB estimate. Memory
measurements remain diagnostics, not an acceptance ceiling or a reason to add
a memory manager. Defer configurable limits and a separate query-index memory
project. Continue the approved state/API simplification with ordinary performance
checks. Keep Windows' existing owned fallback and the existing durability
barriers; Linux measurements do not establish Windows working-set guarantees.

API cleanup does not change the data-migration requirements or owned-view
guarantees. Preserve those while changing method names, types and capabilities.

## Preferred usage model

```text
reader -> immutable snapshot -> graph / closure / union / dataset / metadata
writer -> one pending update -> one published snapshot
```

All reads from one snapshot use its catalog and RDF together. Refresh replaces
the reader's selection; it never changes existing snapshots or escaped views.
Queries obtain fresh datasets when a new selection is wanted. Evaluate removing
public dataset rebinding from 0.7's preferred surface; retain it only for a
demonstrated integration requirement and with explicit iterator semantics.

(Gabe here: If we do support dataset stuff, I would want it to be a similar ilk to the graph with view and copy that we get so we could get like a dataset view and then dataset copy perhaps if we're keeping those CPS. What I'm saying is keep consistent and allow datasets to be used as a return type for graph unions when we compute them. )

User clarification, carried into the API requirements: retained dataset support
must have the same view/copy distinction as graph support. Graph-union operations
must allow dataset results where appropriate. Specify graph-context retention
and duplicate semantics explicitly; a dataset result must not accidentally be
flattened into a graph. Exact API names belong to the Phase 1 surface proposal.

For writes, a normal source update publishes its coherent result before
returning. Explicit grouping of several operations remains available where
needed. Flush and close must not become additional, surprising commit boundaries
in the preferred managed API. Existing configuration setters and close-time
behavior need an inventory and a documented replacement where their semantics
change.

Phase 1 selects the small public surface around reader, writer and snapshot
roles before changing callers. Breaking changes are permitted. The target is
fewer distinct behaviors and entry points, not another set of aliases.

## Phase 1: Make contracts and backend capabilities explicit

First deliverable: a small API-contract change, independent of storage-policy
and state-ownership refactoring.

Work:

- Inventory public lifecycle, query, copy, mutation and maintenance operations
  across Rust, Python and the CLI. Record which are primary, removable,
  adapter-specific or maintenance-only. Keep useful distinctions such as opening
  an existing environment versus creating a new one.
- Replace the assumption that every backend supplies an underlying Oxigraph
  store with an explicit optional/fallible capability. Managed storage must not
  present its empty compatibility scratch store as its authoritative contents.
- Migrate internal callers to graph/query operations or explicit capabilities.
  Avoid introducing a broad plugin framework or speculative trait hierarchy.
- Define consistent view/copy terminology. In particular, document and resolve
  the different meanings of Rust and Python `get_graph`. Apply the same distinction
  to dataset views/copies and specify dataset-returning union operations, per the
  user's note above.
- Clarify that managed storage retains one registration per canonical IRI.
  Specify where discovery/resolution policies select competing source candidates;
  do not imply that multiple stored versions can be queried under one IRI.

The existing `store() -> &Store` cannot report an unsupported operation through
an ordinary compatibility wrapper. Use the approved 0.7 breaking-change boundary
to replace or retire it. An empty store or a new panic is not an acceptable
substitute for a capability error. The first concrete proposal should compare
removing raw-store access with a narrowly named fallible Oxigraph capability;
avoid adding a general capability registry.

Acceptance:

- Supported raw-store access reaches the real store; unsupported access is
  explicit in the chosen API contract.
- Normal graph counts/reads cannot accidentally observe the scratch store.
- Examples and signatures distinguish views, copies, source updates and refresh.
- Focused regressions cover capabilities and source/IRI conflict semantics.

## Phase 2: Complete queries on immutable snapshots

Work:

- Make closure membership, closure views/copies, unions, import explanations,
  aliases and namespace lookup derive from the captured snapshot.
- Provide consistent dataset view/copy behavior for retained dataset operations,
  including dataset results for unions. Preserve the specified named-graph
  contexts and verify view/copy equivalence at the triple or quad level as
  appropriate to the result type.
- Extract shared query logic from the live `OntoEnv` paths. Existing high-level
  methods delegate to that logic; do not add a second closure implementation.
- Keep RDF5D's query engine and existing closure transforms. Preserve imports,
  ontology-declaration and SHACL-prefix behavior, recursion limits, blank-node
  identity, and the distinction between empty and missing graphs.
- Prefer a reader's selected snapshot as the authority for catalog/configuration
  reads, rather than maintaining another independently mutable committed copy.
- Keep close/context-manager semantics limited to releasing the handle's owner.

Acceptance:

- Multiple queries on a retained snapshot agree while another process publishes
  or the original reader refreshes/closes.
- View and copy paths produce the same intended triple sets.
- Metadata-only refresh preserves shared RDF backing and reusable indexes.
- Retained public methods preserve the agreed results. Removed or renamed
  operations have concise migration examples; no compatibility wrapper is
  required solely to preserve the unreleased API.

## Phase 3: Resolve owned bytes versus mmap with a bounded experiment

This can run alongside Phase 2 using the agreed workload bounds. Finish it
before redesigning pin management or committing to a backing-specific internal
refactor.

Compare two isolated implementations with the same public ownership contract:

1. Current immutable files, mmap where supported, and lifetime pins.
2. Fully owned encoded RDF5D bytes per opened version, shared by that process's
   views and iterators. Preserve short selection/load coordination so cleanup
   cannot remove a file during acquisition.

This is an encoded-byte buffer, not an eagerly materialized graph or a second
set of query indexes. The Windows path already uses owned encoded bytes; reuse
that code path. Keep any experimental switch inside the diagnostic harness,
not in the product API.

Reuse `snapshot_profile`, the normal Python API worker, and the existing real
ontology fixtures. Existing approximately 14 MB warm-open diagnostics show similar
owned/mmap timings, but do not establish a multiprocess memory decision.

Measure:

- The approximately 14 MB representative store and approximately 100 MB encoded
  upper-bound store, with 1/4/9 reader processes. Record actual encoded bytes;
  these are workload test points, not an automatic file-size admission limit.
- Initial open, first graph/closure query, warm query and refresh costs, with
  identical integrity checking and index construction.
- Private memory and aggregate PSS on Linux; appropriate private-memory/working-set
  measurements on Windows. Do not add process RSS values and call them shared
  physical-memory usage.
- One and several retained versions across updates, followed by release of the
  last owner. Include independent readers and writer restarts.
- Repeated alternating runs with fixture hashes, implementation revisions and
  native-extension paths recorded. Separate fresh-process and warm-cache results;
  make no cold-storage claim without a controlled cold-cache experiment.

At 100 MB and nine processes, one independently loaded owned buffer per process
allocates about 900 MB for encoded payloads alone. Three distinct retained
versions in every process allocate about 2.7 GB before query indexes, staging and
writer overhead. Cloned views of the same buffer share it. These arithmetic
estimates are not measured RSS or total-memory predictions; the aggregate-memory
experiment must include both the usual single-version case and retained versions.

Original experiment decision rule (the later user clarification above supersedes
budget-driven gating):

- If owned bytes meet the agreed workload budgets, prefer their simpler file
  lifetime model and one platform-independent default.
- If mmap has a material benefit required by the workload, retain the tested pin
  protocol and isolate it inside managed storage.
- Do not ship two equally prominent modes merely because the experiment tested
  both. A product option needs a demonstrated user requirement.

If owned bytes are selected, remove lifetime pins only once every supported
reader is independent of the backing files after loading. Older mmap readers
still need protection. Handle any reader-protocol change through one explicit
offline boundary; do not silently prune files underneath older processes.

Publication atomicity, failure handling and the Windows durability decision
remain necessary whichever backing wins.

## Phase 4: Consolidate managed update state

Work:

- Establish one committed `EnvironmentSnapshot` and one pending candidate owning
  catalog/configuration changes and graph replacements/deletions.
- Consolidate the overlapping responsibilities of `EnvTransaction`,
  `SnapshotGraphIO` checkpoints and `SnapshotUpdate`. Keep only checkpoints that
  are needed for supported nested operations and unwind safety.
- Derive dependency indexes from the candidate or committed catalog. Make their
  ownership/invalidation explicit; avoid independently authoritative copies.
- Retain one publication path and the existing uncertain-outcome/reopen rule.
- Move backend-specific lifecycle and recovery handling out of the normal
  managed mutation path as adapters are separated.

Acceptance:

- A maintainer can identify the sole committed authority and the sole pending
  authority without tracing multiple backends.
- Failed recursive imports, rename, validation and nested updates restore all
  affected graph and metadata state together.
- Uncertain publication is distinguished from a known pre-publication failure;
  cleanup cannot invalidate recovery or turn success into reported rollback.
- The change removes redundant state/branches. It must not merely rename the
  current layers or insert another generic abstraction between them.

## Phase 5: Remove obsolete paths and isolate external-store adapters

Work:

- Remove redundant entry points and obsolete constructor flags from the
  unreleased 0.7 surface. Reuse implementations for retained entry points rather
  than maintaining separate lifecycle paths. Publish a short old-to-new guide.
- Keep legacy managed-format handling migration-only where feasible. Inventory
  the external-store catalog/recovery dependencies before removing old code.
- Give external stores an explicit capability contract. Preserve weaker behavior
  where required; do not imply atomicity with independently committed storage.
- Separate maintenance operations such as migrate, protocol upgrade and prune
  from everyday reader/query documentation.
- Give development-format upgrade helpers a stated support/removal policy
  instead of indefinitely accumulating permanent paths. API removal is separate
  from support for migrating persisted user data.

Acceptance:

- A basic reader/writer example needs no backend selection, recovery knowledge,
  manual flush, or read transaction lifecycle.
- Adapters retain their declared guarantees and fail explicitly for unsupported
  operations.
- Representative Rust, Python and CLI callers migrate to the primary workflow
  using the documented old-to-new mapping.

## Delivery and release gates

Use separate reviewable changes for contracts, shared queries, the backing
experiment/decision, managed-state consolidation, and adapter cleanup. Do not
combine a backing-policy switch with the transaction-state rewrite. Rerun only
the checks appropriate to each change, then the full matrix at integration.

User workflow clarification after Phase 1: keep the remaining phases on the
current `refactor/graph-io-capabilities` branch, using local commits for reviewable
checkpoints, and make one push at the end. Do not create draft PRs or push each
phase. The already-created Phase 1 draft PR #38 has been closed.

For the final revision require:

- Rust/Python/CLI contract coverage, closure/view/copy equivalence and relevant
  custom-store compatibility checks.
- Process tests for selection versus cleanup, retained views and iterators,
  writer restart, reader death, failed refresh and publication interruption.
- Windows directory rename/deletion cases for any retained file-based protocol.
- Explicit migration coverage for any changed reader protocol and legacy format.
- A representative normal-API workload plus the agreed upper-bound memory case.
- Linux/macOS/Windows CI, the documented compiler floor and supported Python
  versions, formatting/lint checks, and documentation checks.
- A written durability contract and release decision for Windows. Process-exit
  tests and retaining a previous file do not establish power-loss recovery.

The final public workflow and state model must be smaller than the starting
point. Passing tests alone is not evidence that consolidation succeeded.

No merge or release is implied by this plan. The first Phase 1 deliverable is
the contract inventory and the implemented fallible `oxigraph_store()` boundary,
using the approved 0.7 breaking-change scope. The deployment
Windows durability contract is accepted and documented: process-crash consistency,
best-effort power-loss durability, with a possible rebuild from sources. The advisory memory
estimate does not block implementation, integration or release.


## Implementation completion

All five consolidation phases are implemented on the approved branch. See
`consolidation-review-2026-10-25.md` for the resulting ownership model, API
removals and final local evidence. Mmap/pins and the Windows owned fallback
remain; no memory-limit manager or index-memory project was added. Dataset
rebinding remains an explicit rdflib integration operation with retained-iterator
coverage. Configuration setters share normal publication/rollback, removing the
old flush/close commit boundary. The final remote matrix must be checked at the
pushed revision. The user accepted the current Windows power-loss limit; its
written contract now explicitly permits a rebuild from ontology sources.
