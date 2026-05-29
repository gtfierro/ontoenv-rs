"""Benchmark ontoenv get_* vs copy_* against rdflib Memory and oxigraph stores.

Loads the Brick 1.4.4 ontology (with its imports closure) and times a handful of
read operations against four rdflib-compatible backends:

- ``ontoenv-get``   — read-only view returned by ``env.get_closure(...)``,
                     backed by the rdf5d on-disk store via mmap.
- ``ontoenv-copy``  — mutable in-memory ``rdflib.Graph`` from ``env.copy_closure(...)``.
- ``rdflib-memory`` — default rdflib ``Memory`` store loaded from the same closure.
- ``oxigraph``      — ``oxrdflib`` store (skipped if ``oxrdflib`` isn't installed).

Run from the ``python/`` directory:

    uv run python bench_rdflib_store.py
    uv run python bench_rdflib_store.py --output ../docs/python-api/_bench_results.txt
"""

from __future__ import annotations

import argparse
import gc
import os
import statistics
import sys
import time
from contextlib import contextmanager
from pathlib import Path

from rdflib import Graph, Dataset, URIRef
from rdflib.namespace import OWL, RDF, RDFS

from ontoenv import OntoEnv


BRICK_IRI = "https://brickschema.org/schema/1.4/Brick"
BRICK_URL = "https://brickschema.org/schema/1.4.4/Brick.ttl"


# ---------- timing helpers ----------------------------------------------------


@contextmanager
def _timed():
    gc.collect()
    t0 = time.perf_counter()
    yield lambda: time.perf_counter() - t0


def bench(fn, *, repeat=3):
    """Run ``fn`` ``repeat`` times, return (best_seconds, result_of_last_run)."""
    times = []
    result = None
    for _ in range(repeat):
        with _timed() as elapsed:
            result = fn()
        times.append(elapsed())
    return min(times), statistics.mean(times), result


# ---------- workloads ---------------------------------------------------------


def count_all_triples(graph):
    n = 0
    for _ in graph.triples((None, None, None)):
        n += 1
    return n


def match_owl_imports(graph):
    n = 0
    for _ in graph.triples((None, OWL.imports, None)):
        n += 1
    return n


# A well-connected class used as the bound endpoint for the subject-only and
# object-only patterns below. As a subject it has its own definition triples;
# as an object it is the target of many rdfs:subClassOf edges.
BRICK_EQUIPMENT = URIRef("https://brickschema.org/schema/Brick#Equipment")


def match_subject_only(graph):
    # (s, ?, ?): no bound predicate, so the sidecar PSO/POS index can't be
    # used today; the closure view falls back to scanning every graph.
    n = 0
    for _ in graph.triples((BRICK_EQUIPMENT, None, None)):
        n += 1
    return n


def match_object_only(graph):
    # (?, ?, o): same fallback as the subject-only case. owl:Class is a
    # high-cardinality object (every class declaration), so the full scan
    # cost dominates regardless of result size.
    n = 0
    for _ in graph.triples((None, None, OWL.Class)):
        n += 1
    return n


SPARQL_TYPE_COUNT = """
SELECT (COUNT(*) AS ?n) WHERE {
  ?s a ?t .
}
"""

SPARQL_SUBCLASS_STAR = """
PREFIX brick: <https://brickschema.org/schema/Brick#>
SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE {
  ?s <http://www.w3.org/2000/01/rdf-schema#subClassOf>* brick:Equipment .
}
"""

SPARQL_LABELS = """
SELECT ?s ?l WHERE {
  ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l .
}
LIMIT 1000
"""


def run_sparql(graph, query):
    return len(list(graph.query(query)))


# ---------- backend setup ----------------------------------------------------


def build_env(env_path: Path, brick_source: str) -> OntoEnv:
    env = OntoEnv(
        path=str(env_path),
        recreate=True,
        offline=False,
        strict=False,
        temporary=False,
    )
    env.add(brick_source)
    env.update()
    env.flush()
    return env


def make_rdflib_memory_from_view(view) -> Graph:
    g = Graph()
    for t in view.triples((None, None, None)):
        g.add(t)
    return g


def make_oxigraph_from_view(view):
    try:
        import oxrdflib  # noqa: F401
    except ImportError:
        return None
    g = Graph(store="Oxigraph")
    for t in view.triples((None, None, None)):
        g.add(t)
    return g


# ---------- reporting --------------------------------------------------------


def fmt_row(label, best, mean, result):
    return f"  {label:<22s} best={best*1000:8.2f} ms  mean={mean*1000:8.2f} ms  result={result}"


def fmt_time(seconds: float) -> str:
    if seconds < 1e-3:
        return f"{seconds*1e6:8.2f} us"
    if seconds < 1.0:
        return f"{seconds*1e3:8.2f} ms"
    return f"{seconds:8.3f}  s"


def render_benchcmp(rows, backend_names, baseline):
    """benchcmp-style pairwise comparison: every backend vs the baseline,
    plus an explicit `ontoenv-get vs oxigraph` block when both are present."""
    if baseline not in backend_names:
        baseline = backend_names[0]

    # Reorganize rows: workload -> backend -> (best, mean, result)
    data: dict[str, dict[str, tuple]] = {}
    workloads: list[str] = []
    for kind, wname, bname, timing, result in rows:
        if kind == "workload":
            workloads.append(wname)
            data[wname] = {}
        else:
            data[wname][bname] = (timing[0], timing[1], result)

    pairs: list[tuple[str, str]] = [
        (other, baseline) for other in backend_names if other != baseline
    ]
    # Extra ontoenv-get vs oxigraph comparison when both backends ran.
    if (
        "ontoenv-get" in backend_names
        and "oxigraph" in backend_names
        and ("ontoenv-get", "oxigraph") not in pairs
    ):
        pairs.append(("ontoenv-get", "oxigraph"))

    out = []
    for other, base_name in pairs:
        out.append(f"\n{other} vs {base_name}")
        out.append(
            f"  {'workload':<32s} {base_name+' best':>16s} {other+' best':>16s}  delta"
        )
        for w in workloads:
            base = data[w].get(base_name)
            cur = data[w].get(other)
            if base is None or cur is None:
                continue
            b_best = base[0]
            c_best = cur[0]
            delta = (c_best - b_best) / b_best * 100.0 if b_best > 0 else float("inf")
            sign = "+" if delta >= 0 else ""
            out.append(
                f"  {w:<32s} {fmt_time(b_best):>16s} {fmt_time(c_best):>16s}  {sign}{delta:7.2f}%"
            )
    return "\n".join(out)


def run_all(env, repeat=3):
    print("Building closure views/graphs...")

    # Read-only view backed by rdf5d/mmap
    view, closure_names = env.get_closure(BRICK_IRI)
    print(f"  closure contains {len(closure_names)} graphs")

    # Mutable in-memory copy via ontoenv
    copy_graph, _ = env.copy_closure(BRICK_IRI)

    # Default rdflib Memory store, populated from the same triples
    rdflib_memory = make_rdflib_memory_from_view(view)

    # Oxigraph store (optional)
    oxigraph_graph = make_oxigraph_from_view(view)

    backends = [
        ("ontoenv-get",   view),
        ("ontoenv-copy",  copy_graph),
        ("rdflib-memory", rdflib_memory),
    ]
    if oxigraph_graph is not None:
        backends.append(("oxigraph", oxigraph_graph))
    else:
        print("  (oxrdflib not installed; skipping oxigraph backend)")

    workloads = [
        ("iterate all triples",         count_all_triples),
        ("match ?s owl:imports ?o",     match_owl_imports),
        ("match Equipment ?p ?o",       match_subject_only),
        ("match ?s ?p owl:Class",       match_object_only),
        ("SPARQL: COUNT rdf:type",      lambda g: run_sparql(g, SPARQL_TYPE_COUNT)),
        ("SPARQL: subClassOf* Equip.",  lambda g: run_sparql(g, SPARQL_SUBCLASS_STAR)),
        ("SPARQL: labels LIMIT 1000",   lambda g: run_sparql(g, SPARQL_LABELS)),
    ]

    rows = []
    for wname, wfn in workloads:
        print(f"\n## {wname}")
        rows.append(("workload", wname, None, None, None))
        for bname, graph in backends:
            best, mean, result = bench(lambda g=graph, fn=wfn: fn(g), repeat=repeat)
            print(fmt_row(bname, best, mean, result))
            rows.append(("row", wname, bname, (best, mean), result))
    return rows, [b[0] for b in backends]


def render_markdown_table(rows, backend_names):
    """Build a per-workload markdown table for embedding in docs."""
    workloads = []
    data = {}
    for kind, wname, bname, timing, result in rows:
        if kind == "workload":
            workloads.append(wname)
            data[wname] = {}
        else:
            data[wname][bname] = (timing, result)

    out = []
    header = "| Workload | " + " | ".join(backend_names) + " |"
    sep = "|" + "---|" * (len(backend_names) + 1)
    out.append(header)
    out.append(sep)
    for w in workloads:
        cells = []
        for b in backend_names:
            entry = data[w].get(b)
            if entry is None:
                cells.append("—")
            else:
                (best, _mean), _result = entry
                cells.append(f"{best*1000:.2f} ms")
        out.append(f"| {w} | " + " | ".join(cells) + " |")
    return "\n".join(out)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--brick",
        default=os.environ.get("BRICK_TTL", BRICK_URL),
        help="Brick source: a URL or local path to Brick.ttl",
    )
    parser.add_argument(
        "--env-path",
        default=".bench-env",
        help="OntoEnv working directory (will be recreated).",
    )
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--baseline", default="rdflib-memory",
                        help="Backend to use as the benchcmp baseline.")
    parser.add_argument("--output", type=Path, default=None,
                        help="If set, write a markdown table of results here.")
    args = parser.parse_args()

    env_path = Path(args.env_path).resolve()
    print(f"Brick source: {args.brick}")
    print(f"Env path:     {env_path}")

    env = build_env(env_path, args.brick)
    try:
        rows, backend_names = run_all(env, repeat=args.repeat)
    finally:
        env.close()

    cmp = render_benchcmp(rows, backend_names, args.baseline)
    print("\n# benchcmp-style comparison")
    print(cmp)

    md = render_markdown_table(rows, backend_names)
    print("\n# Markdown summary\n")
    print(md)

    if args.output:
        args.output.write_text(md + "\n")
        print(f"\nWrote table to {args.output}")


if __name__ == "__main__":
    sys.exit(main())
