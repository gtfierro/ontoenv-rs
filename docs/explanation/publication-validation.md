# Publication validation and acceptance gates

Status: experimental snapshot branch, October 23, 2026. These results describe
the prototype and synthetic fixtures; they are not production performance SLOs
or a proof of durability on every local filesystem. The normal CLI/Python
persistence path is unchanged.

## Platform correctness

The held-open CURRENT regression and the complete feature suite passed on Linux,
macOS and Windows at Rust 1.88 for commit `ac607d9`. Both Windows jobs also passed
ten repetitions of the three-reader/twenty-publication stress test. The original
failure was real: tempfile's MoveFileExW-only replacement returned access denied
while readers were active. `std::fs::rename` uses a Windows POSIX-semantics
fallback, allowing old handles to retain their bytes while new opens select the
replacement. No sleeps or permission-error suppression were added to publication.

All cooperating writers still require the stable writer lock. The tests do not
authorize concurrent external replacement or truncation of mapped graph files.

## Guest crash durability

The disposable QEMU harness completed a nine-case pilot followed by **27 cases
across three repeated cycles**. It used a 64 MiB ext4 image, a Linux
7.2.6-76070206-generic guest, QEMU 8.2.2 with TCG and `cache=none`, and a small
ontology/configuration/alias fixture in an already initialized environment.
The host SIGKILLed QEMU at the selected publication boundary, then booted a fresh
guest. Verification mounted the filesystem read-only after ext4's journal
recovery and opened only `SnapshotReader`, before any application recovery writer.

| Crash boundary | Required result | Three-cycle result |
| --- | --- | --- |
| Object ready; manifest written/synced/directory synced; pointer written/synced | Old version | Old version in every case |
| CURRENT replaced, parent directory not yet synced | Complete old or new version | Old version in all three cases |
| Parent directory synced | New version | New version in all three cases |
| Public commit returned success | New version | New version in all three cases |

Every reopened version had matching graph data, configuration revision and alias
state. The pre-directory-sync result demonstrates why atomic visibility alone
does not establish durable publication. Keep the final directory barrier.

This establishes a bounded **guest kernel/page-cache-loss test** for this
filesystem/kernel setup. The host and physical device caches were not
power-cycled. It does not cover every torn/reordered device write, initialization
interruptions, large-store crash cases, other filesystems or Windows/macOS
power-loss semantics. Directory sync remains a no-op on Windows. Normal-operation
and process-exit correctness there must not be advertised as the same durability
guarantee as this Linux experiment.

Reproduction instructions are in `lib/tests/support/README.md`; the machine-readable
report is `docs/reviews/snapshot-durability-vm-2026-10-23.json`. Guest serial logs
and disk images remain in the local `/tmp` artifact directory identified by that
report. The harness exposes no host directories, network or physical disks.

## Workload measurements

The release diagnostic uses local, offline synthetic sources with long hashed
literals. Four active reader processes repeatedly query their retained version
while the writer makes three separate one-graph commits, followed by one alias-only
commit. Readers then refresh and build a new PSO index while retaining their old
snapshot/index. A burst staged in one update would have one publication; this
experiment deliberately measures three separate publications.

Five fresh-process warm reads were measured per size. The first selective query
builds the PSO index; warm selective queries reuse it. A per-file eviction request
was also made, but POSIX_FADV_DONTNEED is advisory and does not prove cold storage
reads. No host-global cache clearing was performed.

| Measurement | 100 graphs / 13.9 MiB | 100 graphs / 19.5 MiB | 200 graphs / 55.8 MiB |
| --- | ---: | ---: | ---: |
| Warm open, median | 11.5 ms | 15.9 ms | 68.0 ms |
| Open after advisory eviction request | 16.1 ms | 22.8 ms | 79.9 ms |
| First graph copy, warm median | 0.71 ms | 0.96 ms | 2.52 ms |
| First indexed query, warm-file median | 26 ms | 39 ms | 158 ms |
| Three graph-changing commits, range | 230–270 ms | 400–440 ms | 1,039–1,142 ms |
| Metadata-only commit | 63 ms | 25 ms | 24 ms |
| Reader refresh, median across four processes | 26 ms | 36 ms | 104 ms |
| First indexed query after refresh, median | 37 ms | 57 ms | 168 ms |
| Writer peak RSS during update burst | 124 MiB | 178 MiB | 421 MiB |
| Four readers' aggregate PSS, one indexed version each | 128 MiB | 204 MiB | 472 MiB |
| Four readers' aggregate PSS, retaining old and new indexed versions | 245 MiB | 392 MiB | 960 MiB |
| Retained RDF5D bytes after three graph commits and an alias commit | 55.6 MiB | 77.9 MiB | 223.0 MiB |

Held-reader sampled query p95 was approximately 0.039, 0.047 and 0.127 ms
respectively. The maximum observed query was 0.68, 0.36 and 8.9 ms. Those samples
span the ready-to-refresh interval, including setup around the writer burst;
percentiles use each reader's first 4,096 timings, while maxima cover every query.
They are not production tail-latency bounds. Results are single diagnostic runs
per fixture and include normal scheduler/storage variation.

RSS includes shared mappings in each process; aggregate PSS apportions shared
pages. These measurements include one permutation index, not every possible query
index. Memory under more query shapes or more retained versions can be higher.
Logical retained-file lengths exclude filesystem allocation overhead.

The source, commands, process reports and hashes are retained in
`docs/reviews/snapshot-workload-2026-10-23.json`. The worker and controller are
`lib/examples/snapshot_workload.rs` and `lib/tests/support/snapshot_workload.py`.

## Proposed gates before normal API integration

Correctness gates are strict: no mixed versions, no failed cooperating publications
under the tested concurrent-reader workload, retained views stay unchanged, and
failed refresh preserves the old view. Acknowledged commits must survive the
defined durability campaign; a single loss or incomplete selected version fails
that gate. Keep platform durability claims tied to tested/specified flush behavior.

For initial engineering work at roughly 20 MiB with four active readers, use these
**provisional budgets**, subject to workload/product review:

- Graph-changing commit: under one second in the diagnostic workload.
- Refresh: under 100 ms, plus under 100 ms for the first selective query/index.
- Writer peak RSS: under 256 MiB; reader peak RSS under 128 MiB for one indexed
  version and under 256 MiB when deliberately retaining two.
- Retention: continue to expose/document full-store growth per graph-changing
  commit. Do not enable automatic deletion without a process-safe reader protocol.

The target fixture fits these provisional budgets; the larger fixture crosses
some of them. They are not new public latency/memory promises or statistically
validated p95 thresholds. Revisit them with real ontology/query workloads before
making the new publisher the default or expanding its supported durability scope.

## Ownership follow-up and initial Rust integration

The focused ownership review found no cycle requiring a new release mechanism.
Weak-reference tests check that ordinary refresh frees unreferenced old metadata
and RDF5D snapshots, while owned query iterators retain their RDF backing until
dropped. Cloned handles share a snapshot, and metadata-only refresh now preserves
the same mapped RDF5D object and its whole-store indexes. Scoped View indexes
remain view-local to preserve scope-specific property-path semantics.

A paired ~19.5 MiB run with four active readers confirms the difference between
ordinary refresh and intentionally retaining history:

| Reader behavior | Aggregate PSS before | Aggregate PSS after | Refresh median | New index/query median |
| --- | ---: | ---: | ---: | ---: |
| Keep old and new indexed versions | 204 MiB | 392 MiB | 36 ms | 57 ms |
| Release old view before refresh | 204 MiB | 204 MiB | 57 ms | 47 ms |

Ordinary refresh includes destruction of the old state; retained-view refresh
defers that work. All four release-mode workers confirmed the old RDF snapshot
was destroyed before building the new index. File retention remains unchanged.
This is one diagnostic run per mode with independent equivalent fixtures, not
a controlled latency/memory guarantee. Raw worker reports, commands and hashes
are in `docs/reviews/snapshot-ownership-2026-10-23.json`.

The first opt-in integration adds `OntoEnv::create_snapshot`,
`OntoEnv::open_snapshot` and `OntoEnv::open_snapshot_writer`, returning specialized
reader/writer handles. Environment snapshots now expose borrowed graph/scoped
views tied to their selected version. The legacy mutable API and CLI/Python
persistence have not been switched to this backend. Avoid implicit refresh of
retained views: their stable identity is the consistency guarantee. Per-graph
disk COW would not directly remove per-process in-memory query indexes and is
not justified by these measurements alone. Recursive source updates, broader
API adapters, explicit migration and safe cleanup remain integration work.
