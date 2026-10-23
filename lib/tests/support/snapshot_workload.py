"""Linux diagnostic workload: cache hints, fresh readers, query indexes and bursts.

Build the snapshot_workload release example first. Uses only temporary fixtures
below --work; per-file POSIX_FADV_DONTNEED never clears the host's global cache.
It is an eviction request, not proof of cold storage reads. Worker reports are kept.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import selectors
import subprocess
import tempfile


def invoke(binary, *args):
    result = subprocess.run([str(binary), *map(str, args)], capture_output=True,
                            text=True, check=True, timeout=180)
    return json.loads(result.stdout)


def receive(child):
    with selectors.DefaultSelector() as selector:
        selector.register(child.stdout, selectors.EVENT_READ)
        if not selector.select(timeout=180):
            raise TimeoutError(f"Reader {child.pid} did not respond")
        line = child.stdout.readline()
        if not line:
            raise RuntimeError(f"Reader {child.pid} exited: {child.stderr.read()}")
        return json.loads(line)


def memory(children):
    totals = {"rss_kib": 0, "pss_kib": 0}
    for child in children:
        fields = {}
        for line in Path(f"/proc/{child.pid}/smaps_rollup").read_text().splitlines():
            if line.startswith(("Rss:", "Pss:")):
                fields[line.split(":")[0]] = int(line.split()[1])
        totals["rss_kib"] += fields["Rss"]
        totals["pss_kib"] += fields["Pss"]
    return totals


def stored(root):
    objects = list((root / ".ontoenv-snapshots/objects").glob("*/store.r5tu"))
    manifests = list((root / ".ontoenv-snapshots/manifests").glob("*.json"))
    return {"objects": len(objects), "object_bytes": sum(p.stat().st_size for p in objects),
            "manifests": len(manifests), "manifest_bytes": sum(p.stat().st_size for p in manifests)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--cases", default="100:500,100:700,200:1000")
    parser.add_argument("--readers", type=int, default=4)
    parser.add_argument("--burst", type=int, default=3)
    parser.add_argument("--retention", choices=["retain", "release", "both"], default="retain")
    args = parser.parse_args()
    assert args.readers > 0 and args.burst > 0
    args.work.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="snapshot-workload-", dir=args.work)).resolve()
    binary = args.binary.resolve()
    results = {"platform": platform.platform(), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
               "notes": ["Synthetic, local, offline release-build diagnostic; not a controlled hardware benchmark.",
                         "Cache eviction is advisory, not guaranteed cold reads.",
                         "First query builds a PSO index; subsequent queries reuse it.",
                         "Active readers retain their original version while writer publishes.",
                         "Reader samples during updates retain the first 4096 timings; maximum includes all queries.",
                         "RSS sums double-count shared pages; PSS apportions shared pages.",
                         "Retention mode selects whether old query indexes remain owned across refresh."], "cases": []}
    print(f"Artifacts: {work}", flush=True)
    modes = ["retain", "release"] if args.retention == "both" else [args.retention]
    cases = [(int(case.split(":")[0]), int(case.split(":")[1]), mode)
             for case in args.cases.split(",") for mode in modes]
    for graphs, triples, retention in cases:
        root = work / f"{graphs}-{triples}-{retention}"
        prepared = invoke(binary, "prepare", root, graphs, triples)
        initial = stored(root)
        for path in root.rglob("*"):
            if path.is_file():
                fd = os.open(path, os.O_RDONLY)
                try:
                    os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
                finally:
                    os.close(fd)
        evicted = invoke(binary, "read", root)
        warm = [invoke(binary, "read", root) for _ in range(5)]
        children = []
        try:
            for _ in range(args.readers):
                child_mode = "hold" if retention == "retain" else "hold-release"
                children.append(subprocess.Popen([str(binary), "read", str(root), child_mode],
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True))
            ready = [receive(child) for child in children]
            assert all(sample["kind"] == "reader_ready" for sample in ready)
            memory_before = memory(children)
            updated = invoke(binary, "update", root, triples, args.burst)
            for child in children:
                child.stdin.write("refresh\n")
                child.stdin.flush()
            refreshed = [receive(child) for child in children]
            assert all(sample["kind"] == "reader_refreshed" and sample["generation"] == args.burst + 2
                       and sample["retained_generation"] == (1 if retention == "retain" else None)
                       and sample["old_snapshot_released"] == (retention == "release") for sample in refreshed)
            memory_after = memory(children)
            for child in children:
                child.stdin.write("exit\n")
                child.stdin.flush()
            for child in children:
                _, error = child.communicate(timeout=30)
                assert child.returncode == 0, error
            sample = {"graphs": graphs, "triples_per_graph": triples, "readers": args.readers,
                      "retention_mode": retention,
                      "prepare": prepared, "initial_storage": initial, "eviction_requested_read": evicted,
                      "warm_fresh_process_reads": warm, "active_readers_initial": ready,
                      "readers_memory_before": memory_before, "update_burst": updated,
                      "active_readers_refreshed": refreshed, "readers_memory_after": memory_after,
                      "final_storage": stored(root)}
            results["cases"].append(sample)
            (work / "results.json").write_text(json.dumps(results, indent=2) + "\n")
            print(f"{graphs} graphs x {triples}, {retention}: {initial['object_bytes'] / 1048576:.2f} MiB; "
                  f"commit ms {updated['commit_ms']}; reader PSS KiB "
                  f"{memory_before['pss_kib']} -> {memory_after['pss_kib']}", flush=True)
        finally:
            for child in children:
                if child.poll() is None:
                    child.kill()
                child.communicate()
    print(f"Results: {work / 'results.json'}", flush=True)


if __name__ == "__main__":
    main()
