# Owned views and reader-safe cleanup

This change replaces indefinite retention with OS-backed ownership of immutable
RDF files. It preserves the existing publication/flushing protocol and RDF5D
format. No per-graph version store or read transaction was added.

## Lifetime and retention

- An RDF5D file retains an additive lifetime guard. Snapshots, views, raw file
  owners and owned iterators retain that guard until the backing is released.
- Readers hold a short shared acquisition gate across CURRENT selection,
  manifest validation, object pin acquisition and opening. Their long-lived
  shared object pin does not hold the gate or writer lock.
- The sole writer keeps current and exact previous publication roots, skips
  objects with shared pins and retires eligible directories into `trash/` before
  deletion. Startup, successful publication and explicit prune retry cleanup.
  A sleeping reader has no expiry. Process death releases its OS locks.
- Windows requires closing child pin handles before renaming their directories
  ([rename rules](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information)).
  Initialization closes the private candidate before installing and reopening
  it under the writer lock. Cleanup closes its exclusive pin after checking for
  readers and before retirement, while still holding the acquisition gate.
- Failed retirement/deletion is retried; automatic cleanup never changes a
  successful commit into a reported publication failure. After an uncertain
  publication error the writer must reopen before either commit or prune.
- Python supports `OntoEnv.read_snapshot(path)` and snapshot
  `with`/`close()`/`closed`. Closing releases only that handle; escaped views and
  iterators own their backing independently. Rust uses ordinary Arc/drop.

Reader protocol 3 requires an explicit offline upgrade of protocol-2 stores from
this unreleased branch. All older processes must close first: their readers have
no observable pins. Upgrade validates the selected version, provisions lock files
and atomically selects a protocol-3 manifest without rewriting/refetching RDF.
Old protocol 2 had no exact published ancestry, so retention starts at its selected
version. Legacy `.ontoenv/` migration remains separate.

## Validation

Local release validation on Linux:

- Rust workspace tests: 251 passed, eight ignored (fault helpers/VM or ignored
  doctest); includes independent reader iterators surviving writer restarts,
  graceful reader exit and forced process termination, ownership of raw file
  guards, concurrent open/refresh/publication, failed retirement retries,
  acquisition-gate contention, exact previous manifest retention, metadata-only
  sharing, protocol rejection/upgrade and CLI upgrade/prune routing.
- Rebuilt abi3 wheel, isolated Python 3.14 test directory: 168 passed, 21 existing
  rdflib/deprecation warnings. Includes closed-handle errors, scope exceptions,
  escaped graph/iterator ownership, reclamation after the final owner exits and
  read-only prune refusal. Existing URL tests ran with network access.
- Workspace release clippy with all targets and denied warnings; fmt/diff checks;
  Sphinx HTML documentation with warnings treated as errors.
- Dedicated Windows test covers deferred deletion while a non-protocol Windows
  handle denies deletion, followed by retry at writer restart. Windows/macOS
  execution remains for CI; Linux results do not substitute for those checks.

The normal Python workload used 100 graphs, 20,718,528 source bytes and a
14,365,269-byte (13.7 MiB) initial RDF5D object. Graph/closure/dataset queries,
updates, retained views and an in-flight iterator, refresh, alias publication,
parse-error rollback, retry and close assertions all passed. Three full objects
remained while an old view was retained alongside current/previous history;
after process exit the next writer startup reduced this to two objects. The
workload peaked at 447 MiB RSS. Timings from this single run overlap compilation
and test activity and are not a comparative performance claim. Raw results:
`owned-views-workload-2026-10-24.json`.

Full-file graph rewrites, whole-file retention by a long-lived view and the
existing Windows directory-sync limitation remain explicit operating limits.
Keeping previous publication roots is retention, not automatic corruption
fallback or a new power-loss guarantee. CURRENT remains authoritative.
