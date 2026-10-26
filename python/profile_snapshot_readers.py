"""Bounded Linux diagnostic of normal snapshot APIs with independent readers.

`build` archives one Git revision and builds isolated mmap/owned wheels. The
owned copy changes only Snapshot::open_file; no product mode is introduced.
`run` alternates those builds over local mixed-ontology stores and retains three
versions per reader across writer restarts. All memory units in JSON are KiB.
PSS is sampled by the parent after every reader reaches the same barrier.
This is a warm-cache experiment, without an enforced RAM limit or cold-I/O claim.
"""

import argparse
import ctypes
import gc
import hashlib
import json
import os
from pathlib import Path
import platform
import runpy
import selectors
import shutil
import subprocess
import sys
import tarfile
from time import perf_counter

REPO = Path(__file__).resolve().parents[1]
MODEL = "urn:review:building:0"
QUERY = """SELECT (COUNT(*) AS ?n) WHERE {
  ?s a <https://brickschema.org/schema/Brick#Temperature_Sensor> ;
     <http://qudt.org/schema/qudt/unit> ?unit . }"""
PATH_QUERY = """SELECT ?class WHERE {
  ?class <http://www.w3.org/2000/01/rdf-schema#subClassOf>+
    <https://brickschema.org/schema/Brick#Sensor> } LIMIT 100"""


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def emit(value):
    print(json.dumps(value), flush=True)


def timed(operation):
    started = perf_counter()
    value = operation()
    return value, (perf_counter() - started) * 1000


def selected(root):
    directory = root / ".ontoenv-snapshots"
    head = json.loads((directory / "CURRENT").read_text())
    return json.loads((directory / "manifests" / head["manifest"]).read_text())


def memory(pid):
    wanted = {"Rss", "Pss", "Pss_Anon", "Pss_File", "Private_Clean", "Private_Dirty", "Swap"}
    fields = {}
    for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
        name = line.split(":")[0]
        if name in wanted:
            fields[name] = int(line.split()[1])
    fields["private"] = fields["Private_Clean"] + fields["Private_Dirty"]
    return fields


def mapping_count(pid):
    return sum("/store.r5tu" in line for line in Path(f"/proc/{pid}/maps").read_text().splitlines())


def aggregate(children):
    samples = [{"pid": child.pid, "kib": memory(child.pid), "rdf_mappings": mapping_count(child.pid)}
               for child in children]
    for sample in samples:
        sample["peak_rss_kib"] = next(int(line.split()[1]) for line in
            Path(f"/proc/{sample['pid']}/status").read_text().splitlines() if line.startswith("VmHWM:"))
    return {"readers": samples,
            "total_kib": {name: sum(sample["kib"].get(name, 0) for sample in samples)
                          for name in samples[0]["kib"]}}


def build(args):
    work = args.work.resolve()
    work.mkdir()  # Do not overwrite prior evidence.
    source = work / "source"
    source.mkdir()
    archive = work / "source.tar"
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
    subprocess.run(["git", "archive", "--output", str(archive), revision], cwd=REPO, check=True)
    with tarfile.open(archive) as stream:
        stream.extractall(source, filter="data")
    base_python = args.base_python.absolute()
    site = subprocess.check_output([str(base_python), "-I", "-c",
        "import sysconfig; print(sysconfig.get_paths()['purelib'])"], text=True).strip()
    policy = source / "rdf5d/src/snapshot.rs"
    original = policy.read_text()
    old = """        #[cfg(all(feature = \"mmap\", not(windows)))]
        let file = R5tuFile::open_mmap(path)?;
        #[cfg(any(not(feature = \"mmap\"), windows))]
        let file = R5tuFile::open(path)?;
        Ok(file)"""
    assert original.count(old) == 1, "Review the backing patch against this revision"
    report = {"revision": revision, "source_archive_sha256": sha256(archive),
              "base_python": str(base_python), "dependency_site": site, "variants": {},
              "policy_change": {"file": "rdf5d/src/snapshot.rs", "old": old,
                                "new": "        R5tuFile::open(path)"}}
    for mode in ("mmap", "owned"):
        if mode == "owned":
            policy.write_text(original.replace(old, "        R5tuFile::open(path)"))
        wheels = work / mode / "wheels"
        wheels.mkdir(parents=True)
        command = [str(args.maturin.absolute()), "build", "--release", "--locked",
                   "--manifest-path", "python/Cargo.toml", "--features", "abi3,cli", "--out", str(wheels)]
        environment = dict(os.environ, CARGO_TARGET_DIR=str(args.target_dir.resolve()))
        with (work / f"build-{mode}.log").open("w") as log:
            subprocess.run(command, cwd=source, env=environment, check=True, stdout=log, stderr=log)
        wheel, = wheels.glob("*.whl")
        venv = work / mode / "venv"
        subprocess.run([str(base_python), "-m", "venv", str(venv)], check=True)
        python = venv / "bin/python"
        subprocess.run(["uv", "pip", "install", "--python", str(python), "--no-deps", str(wheel)],
                       env=dict(os.environ, UV_CACHE_DIR=str(work / "uv-cache")), check=True)
        purelib = Path(subprocess.check_output([str(python), "-I", "-c",
            "import sysconfig; print(sysconfig.get_paths()['purelib'])"], text=True).strip())
        # Reuse the same dependency versions in both isolated environments.
        # Their own ontoenv package precedes this path; workers verify its path.
        (purelib / "profile-dependencies.pth").write_text(site + "\n")
        native = Path(subprocess.check_output([str(python), "-I", "-c",
            "import ontoenv._native; print(ontoenv._native.__file__)"], text=True).strip()).resolve()
        assert native.is_relative_to(venv)
        report["variants"][mode] = {"python": str(python), "native": str(native),
            "native_sha256": sha256(native), "wheel_sha256": sha256(wheel),
            "policy_source_sha256": sha256(policy), "build_command": command}
        (work / "builds.json").write_text(json.dumps(report, indent=2) + "\n")
        print(f"Built {mode}: {native}", flush=True)


def model_function():
    return runpy.run_path(str(REPO / "python/bench_managed_ontology.py"))["model"]


def prepare(args):
    from ontoenv import OntoEnv

    root = args.root.resolve()
    root.mkdir()
    sources = root / "sources"
    sources.mkdir()
    fixture = REPO / "lib/fixtures/brick-stuff"
    real = [fixture / "Brick-1.3.ttl", *sorted((fixture / "support").glob("*.ttl"))]
    for path in real:
        shutil.copyfile(path, sources / path.name)
    model = model_function()
    for number in range(100 - len(real)):
        (sources / f"building-{number}.ttl").write_text(model(number, args.nodes))
    with OntoEnv.connect(root, offline=True, strict=False, search_directories=[str(sources)]) as writer:
        _, elapsed = timed(writer.update)
        assert len(writer.get_ontology_names()) == 100
    record = selected(root)
    emit({"root": str(root), "nodes": args.nodes, "graphs": 100,
          "encoded_bytes": record["object_length"], "update_ms": elapsed,
          "source_bytes": sum(path.stat().st_size for path in sources.iterdir()),
          "source_sha256": {path.name: sha256(path) for path in sorted(sources.iterdir())},
          "object_sha256": sha256(root / ".ontoenv-snapshots/objects" / record["object_id"] / "store.r5tu")})


def publish(args):
    from ontoenv import OntoEnv

    source = args.root / "building-0.ttl"
    source.write_text(model_function()(0, args.nodes, args.revision))
    # Each invocation opens and closes a writer: readers span real restarts.
    with OntoEnv.open(args.root) as writer:
        _, elapsed = timed(lambda: writer.add(str(source), overwrite=True, fetch_imports=False))
        info = writer.snapshot().info
    emit({"generation": info["generation"], "publish_ms": elapsed,
          "object_id": selected(args.root)["object_id"]})


def prune(args):
    from ontoenv import OntoEnv

    with OntoEnv.open(args.root) as writer:
        report = writer.prune()
    emit(report)


class ReaderState:
    def __init__(self, args):
        from ontoenv import OntoEnv
        import ontoenv._native

        self.args = args
        self.retained = []
        self.current = None
        self.reader, elapsed = timed(lambda: OntoEnv.open(args.root, read_only=True))
        native = Path(ontoenv._native.__file__).resolve()
        assert native.is_relative_to(Path(sys.prefix).resolve()), "Source-tree extension shadowed the wheel"
        assert bool(mapping_count(os.getpid())) == (args.mode == "mmap")
        emit({"phase": "opened", "open_ms": elapsed, "native": str(native),
              "generation": self.reader.snapshot().info["generation"], "python": sys.version})

    def query(self, revision):
        from rdflib import Literal, URIRef

        snapshot = self.reader.snapshot()
        graph, graph_ms = timed(lambda: snapshot.view_graph(MODEL))
        result, graph_query_ms = timed(lambda: list(graph.query(QUERY)))
        assert int(result[0].n) == self.args.nodes
        (closure, names), closure_ms = timed(lambda: snapshot.view_closure(MODEL))
        result, first_query_ms = timed(lambda: list(closure.query(QUERY)))
        assert int(result[0].n) == self.args.nodes
        warm = [timed(lambda: list(closure.query(QUERY)))[1] for _ in range(3)]
        paths, path_ms = timed(lambda: list(closure.query(PATH_QUERY)))
        assert paths
        iterator = graph.triples((None, None, None))
        first = next(iterator)
        assert (URIRef(MODEL), URIRef("urn:review:revision"), Literal(str(revision))) in graph
        self.current = (snapshot, graph, closure, iterator, first, revision)
        return {"generation": snapshot.info["generation"], "closure_graphs": len(names),
                "graph_ms": graph_ms, "first_graph_query_ms": graph_query_ms,
                "closure_ms": closure_ms, "first_closure_query_ms": first_query_ms,
                "warm_closure_query_ms": warm, "path_query_ms": path_ms}

    def refresh(self, revision):
        self.retained.append(self.current)
        changed, elapsed = timed(self.reader.refresh)
        assert changed
        result = self.query(revision)
        from rdflib import Literal, URIRef
        for _, graph, _, _, _, old_revision in self.retained:
            assert (URIRef(MODEL), URIRef("urn:review:revision"), Literal(str(old_revision))) in graph
        return dict(result, refresh_ms=elapsed)

    def release(self):
        # Retained iterators must finish with their original term dictionary.
        from rdflib import Literal, URIRef
        counts = []
        for _, _, _, iterator, first, revision in self.retained:
            rows = {first, *iterator}
            assert (URIRef(MODEL), URIRef("urn:review:revision"), Literal(str(revision))) in rows
            counts.append(len(rows))
        self.retained.clear()
        gc.collect()
        return {"released_versions": len(counts), "old_iterator_rows": counts}


def worker(args):
    state = ReaderState(args)
    for line in sys.stdin:
        command = line.strip()
        if command == "query":
            result = state.query(0)
        elif command.startswith("refresh "):
            result = state.refresh(int(command.split()[1]))
        elif command == "release":
            result = state.release()
        elif command == "trim":
            # Diagnostic only: distinguish allocator retention from live owners.
            library = ctypes.CDLL(None)
            trim = library.malloc_trim
            trim.argtypes = [ctypes.c_size_t]
            trim.restype = ctypes.c_int
            result = {"malloc_trim": trim(0)}
        elif command == "close":
            state.retained.clear()
            state.current = None
            state.reader.close()
            gc.collect()
            assert mapping_count(os.getpid()) == 0
            result = {"rdf_mappings": 0}
        elif command == "exit":
            break
        else:
            raise ValueError(command)
        emit(dict(result, phase=command))


def receive(child, timeout=180):
    with selectors.DefaultSelector() as selector:
        selector.register(child.stdout, selectors.EVENT_READ)
        if not selector.select(timeout):
            raise TimeoutError(f"reader {child.pid}")
        line = child.stdout.readline()
        if not line:
            raise RuntimeError(f"reader {child.pid} exited; inspect its stderr artifact")
        return json.loads(line)


def invoke(python, *arguments):
    result = subprocess.run([str(python), "-I", str(Path(__file__).resolve()), *map(str, arguments)],
                            capture_output=True, text=True, timeout=300, check=True)
    return json.loads(result.stdout)


def run(args):
    assert platform.system() == "Linux", "PSS/mapping diagnostics require Linux"
    work = args.work.resolve()
    work.mkdir()
    builds = json.loads(args.builds.read_text())
    variants = builds["variants"]
    for variant in variants.values():
        assert sha256(Path(variant["native"])) == variant["native_sha256"]
    report = {"builds": builds, "platform": platform.platform(), "harness_sha256": sha256(Path(__file__)),
              "notes": ["Warm-cache diagnostic; no cold-I/O or hard-limit claim.",
                        "PSS is summed at synchronized barriers; RSS sums double-count shared mappings.",
                        "Owned build retains identical pinning and strict integrity checks.",
                        "Three independently encoded versions per process; cloned views share a version.",
                        "Diagnostic malloc_trim samples do not represent production behavior.",
                        "Reader memory excludes the separate writer, which exits before sampling."],
              "fixtures": [], "runs": []}
    output = work / "results.json"
    def save():
        output.write_text(json.dumps(report, indent=2) + "\n")
    cases = [int(value) for value in args.nodes.split(",")]
    for nodes in cases:
        fixture = invoke(variants["mmap"]["python"], "prepare", "--root", work / f"fixture-{nodes}", "--nodes", nodes)
        report["fixtures"].append(fixture)
        save()
        print(f"Fixture {nodes}: {fixture['encoded_bytes'] / 1048576:.2f} MiB", flush=True)
        for count in map(int, args.readers.split(",")):
            for repeat in range(args.repeats):
                order = ("mmap", "owned") if repeat % 2 == 0 else ("owned", "mmap")
                for mode in order:
                    root = work / f"case-{nodes}-{count}-{repeat}-{mode}"
                    root.mkdir()
                    shutil.copytree(Path(fixture["root"]) / ".ontoenv-snapshots", root / ".ontoenv-snapshots")
                    python = variants[mode]["python"]
                    children, logs = [], []
                    sample = {"nodes": nodes, "readers": count, "repeat": repeat, "mode": mode,
                              "root": str(root), "encoded_bytes": fixture["encoded_bytes"], "phases": {}}
                    report["active_run"] = sample
                    try:
                        for index in range(count):
                            log = (root / f"reader-{index}.stderr").open("w")
                            logs.append(log)
                            children.append(subprocess.Popen([python, "-I", str(Path(__file__).resolve()),
                                "worker", "--root", str(root), "--nodes", str(nodes), "--mode", mode],
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True))
                        def collect(name):
                            replies = [receive(child) for child in children]
                            sample["phases"][name] = dict(aggregate(children), replies=replies)
                            save()
                        def command(value, name=None):
                            for child in children:
                                child.stdin.write(value + "\n")
                                child.stdin.flush()
                            collect(name or value)
                        collect("opened")
                        command("query", "queried")
                        for revision in (1, 2):
                            update = invoke(python, "publish", "--root", root, "--nodes", nodes, "--revision", revision)
                            sample.setdefault("publications", []).append(update)
                            command(f"refresh {revision}", f"versions-{revision + 1}")
                        initial_object = selected(Path(fixture["root"]))["object_id"]
                        old_path = root / ".ontoenv-snapshots/objects" / initial_object / "store.r5tu"
                        assert old_path.exists(), "Retained readers lost their first object"
                        command("release")
                        sample["prune_after_release"] = invoke(python, "prune", "--root", root)
                        assert not old_path.exists(), "Released readers still pin the oldest object"
                        command("trim", "released_after_diagnostic_trim")
                        command("close")
                        command("trim", "closed_after_diagnostic_trim")
                        for child in children:
                            child.stdin.write("exit\n")
                            child.stdin.flush()
                            child.communicate(timeout=30)
                            assert child.returncode == 0
                        report["runs"].append(sample)
                        del report["active_run"]
                        save()
                        phases = sample["phases"]
                        print(f"{nodes} / {count} readers / {mode} / {repeat}: PSS MiB "
                              + " -> ".join(f"{phases[name]['total_kib']['Pss'] / 1024:.1f}" for name in
                                  ("opened", "queried", "versions-3", "released_after_diagnostic_trim", "closed_after_diagnostic_trim")), flush=True)
                    finally:
                        for child in children:
                            if child.poll() is None:
                                child.kill()
                            child.communicate()
                        for log in logs:
                            log.close()
    print(f"Results: {output}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    item = commands.add_parser("build")
    item.add_argument("--work", type=Path, required=True)
    item.add_argument("--maturin", type=Path, required=True)
    item.add_argument("--base-python", type=Path, required=True)
    item.add_argument("--target-dir", type=Path, required=True)
    item.set_defaults(function=build)
    item = commands.add_parser("run")
    item.add_argument("--work", type=Path, required=True)
    item.add_argument("--builds", type=Path, required=True)
    item.add_argument("--nodes", default="350,3000")
    item.add_argument("--readers", default="1,4,9")
    item.add_argument("--repeats", type=int, default=2)
    item.set_defaults(function=run)
    for name, function in (("prepare", prepare), ("publish", publish), ("prune", prune), ("worker", worker)):
        item = commands.add_parser(name)
        item.add_argument("--root", type=Path, required=True)
        if name != "prune":
            item.add_argument("--nodes", type=int, required=True)
        if name == "publish":
            item.add_argument("--revision", type=int, required=True)
        if name == "worker":
            item.add_argument("--mode", choices=("mmap", "owned"), required=True)
        item.set_defaults(function=function)
    args = parser.parse_args()
    args.function(args)


if __name__ == "__main__":
    main()
