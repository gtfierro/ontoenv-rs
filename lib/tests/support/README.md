# Snapshot durability and workload diagnostics

These opt-in helpers use temporary fixture directories and disposable VM images.
They are not part of normal `cargo test` or a public API. The current reports are
in `docs/reviews/snapshot-durability-vm-2026-10-23.json` and
`docs/reviews/snapshot-workload-2026-10-23.json`.

## Guest crash/reboot matrix

Requirements: Linux host, QEMU x86_64, a Linux kernel with built-in ext4,
virtio-blk/PCI and serial console support, **static** busybox, cpio, mkfs.ext4,
Python 3, and a Linux unit-test executable. KVM and root access are not required.
Supply a kernel you can read, for example by downloading and extracting its
distribution package into `/tmp`; no host package installation is necessary.

Build the executable and use the path Cargo prints:

```sh
cargo test -p ontoenv --locked --release --features transactional-snapshots --lib --no-run
python3 lib/tests/support/snapshot_vm.py \
  --kernel /path/to/vmlinuz \
  --probe /path/to/the/ontoenv-unit-test-executable \
  --work /tmp/ontoenv-durability-vm --cycles 3
```

The script creates a fresh work directory, initramfs and 64 MiB ext4 disk image.
It copies the test executable and its dynamic libraries into the initramfs. No
host directory, network or physical disk is exposed to guests. All guest writes
go to the disposable image. It seeds and syncs an environment, then clones that
baseline for each publication phase. It SIGKILLs QEMU when the guest reports the
phase, discarding the guest's unflushed state. A fresh boot mounts the filesystem
read-only (ext4 may replay its journal) and opens only `SnapshotReader`.

Before CURRENT replacement, the old version must remain. After replacement but
before directory sync, either complete version is allowed. After directory sync
or a successful public commit, the new version must survive. Graph contents,
configuration and alias state must agree. The nine phases are repeated per cycle.
The harness retains serial logs, disk images and `results.json` under `--work`.

This tests **guest kernel/page-cache loss** with QEMU `cache=none`. It does not
power-cycle host/device caches, simulate every reordered or torn device write,
test other filesystems, or cover environment initialization interruptions. The
fixture is small; this is a publication protocol check.

## Multi-process workload

Requirements: Linux, Python 3, and the release example:

```sh
cargo build -p ontoenv --locked --release --features transactional-snapshots --example snapshot_workload
python3 lib/tests/support/snapshot_workload.py \
  --binary target/release/examples/snapshot_workload \
  --work /tmp/ontoenv-benchmark-review
```

Defaults are 100 graphs × 500 triples (~14 MiB), 100 × 700 (~20 MiB), and
200 × 1,000 (~56 MiB), with four active reader processes and three separately
committed one-graph updates. Override `--cases`, `--readers` and `--burst` as needed.
All sources are generated locally and OntoEnv is offline.

The diagnostic requests per-file cache eviction with POSIX_FADV_DONTNEED; this
is advisory, not proof of cold reads. It never clears the host's global caches.
Five warm fresh-process opens follow, each measuring the first graph read,
first selective query/PSO index construction and repeated warm queries.

Active readers then repeatedly query their retained version during the writer's
burst. Afterward they explicitly refresh and build a new query index while still
retaining the old snapshot. The script checks old graph stability, records reader
RSS/PSS and writer peak RSS, and reports retained object/manifest lengths.
The first 4,096 query timings per active reader are kept for percentiles; the
maximum covers every query. RSS sums count shared mappings repeatedly; PSS
apportions shared pages. These are diagnostics, not controlled performance SLOs.

Use `--retention both` to compare that deliberate two-version case with ordinary
refresh after releasing old handles. Each mode uses an independent fixture.
Release-mode workers check a weak reference to ensure the old RDF snapshot has
been destroyed before indexing the refreshed version. The paired target-size
report is `docs/reviews/snapshot-ownership-2026-10-23.json`:

```sh
python3 lib/tests/support/snapshot_workload.py \
  --binary target/release/examples/snapshot_workload \
  --work /tmp/ontoenv-benchmark-review --cases 100:700 --retention both
```
