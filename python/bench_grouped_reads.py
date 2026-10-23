"""Offline RDF5D grouped-read benchmark.

Run after ``maturin develop --release`` from python/:
    .venv/bin/python bench_grouped_reads.py --triples-per-graph 3000 --repeat 30

The generated Turtle fixture has three import-linked named graphs, one unrelated
graph, and a triple shared by all graphs. The fixed IRIs make query results
comparable across runs. Output is JSON for before/after comparison.
"""

from __future__ import annotations

import argparse
import json
import resource
import statistics
import tempfile
import time
from pathlib import Path

from rdflib import URIRef
from rdflib.namespace import OWL

from ontoenv import OntoEnv


ROOT = "urn:bench:root"
MID = "urn:bench:mid"
LEAF = "urn:bench:leaf"
OTHER = "urn:bench:other"
S = URIRef("urn:bench:s:42")
P = URIRef("urn:bench:p:rare")
O = URIRef("urn:bench:o:42")


def make_fixture(directory: Path, triples_per_graph: int) -> list[Path]:
    paths = []
    for index, name in enumerate((ROOT, MID, LEAF, OTHER)):
        lines = [f"<{name}> a <{OWL.Ontology}> ."]
        if index < 2:
            lines.append(f"<{name}> <{OWL.imports}> <{(MID, LEAF)[index]}> .")
        lines.append("<urn:bench:shared> <urn:bench:p:shared> <urn:bench:o:shared> .")
        for n in range(triples_per_graph):
            predicate = P if n == 42 else URIRef(f"urn:bench:p:{n % 19}")
            lines.append(
                f"<urn:bench:s:{n}> <{predicate}> <urn:bench:o:{n % 97}> ."
            )
        path = directory / f"graph-{index}.ttl"
        path.write_text("\n".join(lines) + "\n")
        paths.append(path)
    return paths


def peak_kib() -> int:
    return resource.getrusage(resource.RUSAGE_SELF).ru_maxrss


def bench(graph, pattern, repeat: int, *, raw: bool = False) -> dict:
    def run():
        rows = graph.store.triples(pattern, None) if raw else graph.triples(pattern)
        return sum(1 for _ in rows)

    result = run()  # warm term and graph caches
    times = []
    for _ in range(repeat):
        start = time.perf_counter_ns()
        assert run() == result
        times.append((time.perf_counter_ns() - start) / 1e6)
    return {"rows": result, "median_ms": statistics.median(times)}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--triples-per-graph", type=int, default=3000)
    parser.add_argument("--repeat", type=int, default=30)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="ontoenv-grouped-bench-") as temporary:
        directory = Path(temporary)
        paths = make_fixture(directory, args.triples_per_graph)
        env = OntoEnv(path=directory / "environment", recreate=True, offline=True)
        try:
            for path in paths:
                env.add(str(path), fetch_imports=False)
            env.flush()
            before_kib = peak_kib()
            start = time.perf_counter_ns()
            dataset = env.get_dataset()
            dataset_bind_ms = (time.perf_counter_ns() - start) / 1e6
            after_dataset_kib = peak_kib()
            start = time.perf_counter_ns()
            closure, names = env.get_closure(ROOT)
            closure_bind_ms = (time.perf_counter_ns() - start) / 1e6
            patterns = {
                "subject": (S, None, None),
                "predicate": (None, P, None),
                "object": (None, None, O),
                "predicate_object": (None, P, O),
                "unbound": (None, None, None),
            }
            results = {
                "fixture": {
                    "triples_per_graph": args.triples_per_graph,
                    "named_graphs": 4,
                    "closure_names": sorted(names),
                    "shared_triple_in_all_graphs": True,
                    "offline": True,
                },
                "repeat": args.repeat,
                "bind_ms": {"dataset": dataset_bind_ms, "closure": closure_bind_ms},
                "peak_kib": {
                    "before_bind": before_kib,
                    "after_dataset_bind": after_dataset_kib,
                    "after_queries": None,
                },
                "dataset": {name: bench(dataset, pat, args.repeat, raw=True) for name, pat in patterns.items()},
                "closure": {name: bench(closure, pat, args.repeat) for name, pat in patterns.items()},
            }
            results["peak_kib"]["after_queries"] = peak_kib()
            print(json.dumps(results, indent=2, sort_keys=True))
        finally:
            env.close()


if __name__ == "__main__":
    main()
