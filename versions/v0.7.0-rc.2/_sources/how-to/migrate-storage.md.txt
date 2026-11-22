# Migrate to the 0.7 storage backend

New managed persistent environments use `.ontoenv-snapshots/`. Opening a legacy
`.ontoenv/` environment fails with a migration instruction; reading it never
silently changes the format. Temporary environments and custom `graph_store=`
backends keep their existing storage behavior.

Close every process using the legacy environment, including read-only handles.
Run this from its root or a child directory:

```sh
ontoenv migrate
```

Or use Python:

```python
from ontoenv import OntoEnv

with OntoEnv.migrate("./ontology-env") as env:
    print(env.snapshot().info)
    print(env.get_ontology_names())
```

Rust exposes `OntoEnv::migrate(root)` and `SnapshotWriter::migrate(root)`.
Migration acquires the legacy exclusive lock and the new writer lock, checks for
interrupted mutations and mismatched catalog/store revisions, then copies the
stored graphs, metadata, aliases and configuration. It does not fetch or reparse
ontology sources. Only a complete validated generation zero is installed.
The original `.ontoenv/` files remain in place; an operational `store.lock` is
created if the old release did not provide it. Source file contents are unchanged.

Preparation or validation failures leave the new directory uninstalled. Resolve
the reported legacy inconsistency before retrying. An I/O error after the final
directory rename can leave an installed version with uncertain durability;
open it to inspect the selection before retrying. An interrupted legacy mutation requires
`ontoenv recover` (or recovery using the old release) first. Duplicate canonical
IRIs cannot be represented by the new one-graph-per-IRI catalog; normalize those
registrations with the old release before migrating. A JSON-only catalog has no
persisted graph revision, so migration requires exact graph-name agreement with
the stored data and rejects catalog-only empty graph registrations.

After migration, current APIs select the new directory even though `.ontoenv/`
remains. Repeating migration fails; it never replaces the new environment.
Old releases can still open the retained legacy directory, but they see only
its pre-migration state. Writes to that retained copy are not synchronized with
the new backend. Keep it as a rollback copy rather than running both versions
as writers to the same logical environment.

For rollback, stop all handles first, keep a separate copy of the new directory,
and restore/use the legacy directory with the old release. There is no automatic
downgrade of changes made after migration. Never delete objects from a running
snapshot environment: old views may still map them. Reset and recreation publish
new versions; cleanup respects all live views.

Opening a copied environment uses its new root for publication; it does not
rewrite stored absolute ontology source paths or search directories.

A managed read-only handle now opens while the writer remains active. It keeps
its selected version until `reader.refresh()`; old returned snapshots and views
keep theirs. `writer.update()` reads sources and publishes changes.
`reader.refresh()` selects a publication without accessing sources.
`env.refresh_dataset(dataset)` explicitly rebinds that dataset to the handle's
selected version. `flush()` does not advance readers or update old managed views.

Graph changes currently write a complete RDF5D generation. Metadata-only commits
reuse the existing artifact and indexes. Cleanup retains the current and previous
published versions and every object pinned by live views. Allow space for these
versions plus one full candidate during graph-changing writes. A long-lived view
can retain a full old store until released; deletion retries can require more space.
See [the snapshot guide](../explanation/transactional-snapshots.md) for memory,
write-cost and durability measurements. Unix directory syncing is implemented;
Windows directory syncing remains unsupported. The accepted Windows 0.7 contract
provides process-crash consistency with best-effort power-loss durability. A
power failure can lose an update or require rebuilding from ontology sources;
retaining a previous version does not guarantee its recovery. See the snapshot
guide for the full contract. Power-loss claims on other platforms remain limited
to the tested filesystems and barriers described in the validation report.

## Upgrade earlier 0.7 development stores

Earlier builds of this unreleased branch use reader protocol 2 and retain files
indefinitely. The new protocol 3 enables cleanup. Ordinary opens reject protocol
2 with an upgrade instruction; there is no silent upgrade.

Close **all** processes using that store, including read-only processes and
escaped views, before running this from its root or a child directory:

```sh
ontoenv upgrade
```

Python provides `OntoEnv.upgrade(path)` (returns a writer); Rust provides
`OntoEnv::upgrade(root)` and `SnapshotWriter::upgrade(root)`. The writer lock
detects another writer, but older readers have no pins, so the operator must
ensure they are closed. Afterward, use only protocol-3-capable binaries.

The upgrade validates the selected manifest and graph, provisions reader pins,
and atomically selects a new manifest without rewriting/refetching graph data.
An interrupted upgrade before selection can be retried. After an uncertain
publication error, try opening the store to inspect whether upgrade completed.
Protocol 2 recorded no exact manifest ancestry: retention starts at the selected
version; older unreferenced versions become eligible for cleanup. Back up the
closed directory beforehand if those versions are needed. This protocol upgrade
is separate from `migrate`, which copies legacy `.ontoenv/` stores.

## Rust adapter API changes

In 0.7, `GraphIO::store() -> &Store` is replaced by the fallible
`GraphIO::oxigraph_store() -> Result<&Store>`. In-memory and caller-supplied
Oxigraph backends return their actual store. Managed snapshots, lazy legacy
RDF5D backends and Python adapters return `StoreCapabilityError`, including after
a legacy graph has been loaded into its private cache.

For backend-independent access, use `io.graph_ids()`, `io.size()`, graph/union
operations, or a managed snapshot. `EnvironmentSnapshot::view_graph()` borrows
a query view; Rust `get_graph()`/`copy_graph()` materialize owned graphs whose
mutations do not affect the environment. Python `get_graph()` continues to
return a read-only view.

Custom Rust adapters with an authoritative Oxigraph store can implement
`oxigraph_store()` as `Ok(&self.store)` and use the Oxigraph-backed defaults.
Other adapters implement graph operations directly. Move any former
`ensure_loaded()` behavior into the adapter's graph-read implementation; that
trait hook is removed. A scratch or partial cache is not an authoritative raw
store. Direct Oxigraph writes bypass the catalog and require the adapter's
documented reconciliation procedure.

These API changes do not change the on-disk reader protocol or require another
storage upgrade.
