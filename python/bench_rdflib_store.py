"""Benchmark ontoenv get_* vs copy_* against rdflib Memory and oxigraph stores.

Loads the Brick 1.4.4 ontology (with its imports closure) and times a handful of
read operations against four rdflib-compatible backends:

- ``ontoenv-get``   — read-only, zero-copy view returned by ``env.get_closure(...)``,
                     served directly from the rdf5d mmap snapshot.
- ``ontoenv-copy``  — mutable in-memory ``rdflib.Graph`` from ``env.copy_closure(...)``.
- ``rdflib-memory`` — default rdflib ``Memory`` store, loaded from the ``copy_closure``
                     graph so it holds the identical (cleaned, flattened) triple set.
- ``oxigraph``      — ``oxrdflib`` store, also loaded from ``copy_closure`` (skipped if
                     ``oxrdflib`` isn't installed).

All four backends therefore hold the same triples: ``get_closure`` now applies
the same closure transforms as ``copy_closure`` (see ``OntoEnv.get_closure``).

Run from the ``python/`` directory:

    uv run python bench_rdflib_store.py
    uv run python bench_rdflib_store.py --output ../docs/explanation/_bench_results.txt
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


def bench(fn, *, repeat=3, warmup=True):
    """Run ``fn`` ``repeat`` times, return (mean_seconds, stddev_seconds, result_of_last_run).

    A single untimed warm-up run is executed first when ``warmup`` is set, so
    the measured mean is not dominated by one-off first-iteration costs
    (cold term caches, lazy index builds, etc.). This is also fairer to the
    read-only ``ontoenv-get`` backend: ``copy_closure``'s equivalent cold
    cost (materializing the rdflib graph) happens outside the timed region in
    :func:`run_all`, so timing ``ontoenv-get`` cold would compare apples to
    oranges. Pass ``warmup=False`` to include the cold run.
    """
    if warmup:
        fn()
    times = []
    result = None
    for _ in range(repeat):
        with _timed() as elapsed:
            result = fn()
        times.append(elapsed())
    mean = statistics.mean(times)
    stddev = statistics.stdev(times) if len(times) > 1 else 0.0
    return mean, stddev, result


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
    # (s, ?, ?): subject bound, served from the SPO permutation index.
    n = 0
    for _ in graph.triples((BRICK_EQUIPMENT, None, None)):
        n += 1
    return n


def match_object_only(graph):
    # (?, ?, o): object bound, served from the OSP permutation index. owl:Class
    # is a high-cardinality object (every class declaration), so result size
    # dominates the cost.
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


def make_rdflib_memory_from(source) -> Graph:
    """Populate a default rdflib Memory store with the same triples as ``source``."""
    g = Graph()
    for t in source.triples((None, None, None)):
        g.add(t)
    return g


def make_oxigraph_from(source):
    """Populate an oxrdflib store with the same triples as ``source``."""
    try:
        import oxrdflib  # noqa: F401
    except ImportError:
        return None
    g = Graph(store="Oxigraph")
    for t in source.triples((None, None, None)):
        g.add(t)
    return g


# ---------- reporting --------------------------------------------------------


def fmt_row(label, mean, stddev, result):
    return f"  {label:<22s} mean={mean*1000:8.2f} ± {stddev*1000:6.2f} ms  result={result}"


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

    # Reorganize rows: workload -> backend -> (mean, stddev, result)
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
            b_mean = base[0]
            c_mean = cur[0]
            delta = (c_mean - b_mean) / b_mean * 100.0 if b_mean > 0 else float("inf")
            sign = "+" if delta >= 0 else ""
            out.append(
                f"  {w:<32s} {fmt_time(b_mean):>16s} {fmt_time(c_mean):>16s}  {sign}{delta:7.2f}%"
            )
    return "\n".join(out)


def run_all(env, repeat=3):
    print("Building closure views/graphs...")

    # Read-only, zero-copy view backed by the rdf5d mmap snapshot. This is a
    # closure view: it applies the same closure transforms as copy_closure
    # (strip resolved owl:imports, collapse ontology declarations onto the
    # root, consolidate SHACL prefixes) and presents a single flattened,
    # de-duplicated graph, so its triple set matches the materialized
    # backends below.
    view, closure_names = env.get_closure(BRICK_IRI)
    print(f"  closure contains {len(closure_names)} graphs")

    # Mutable in-memory copy via ontoenv. copy_closure materializes the same
    # cleaned, flattened closure.
    copy_graph, _ = env.copy_closure(BRICK_IRI)

    # Default rdflib Memory and oxigraph stores, populated from the SAME
    # cleaned closure so all four backends hold identical triples and their
    # counts match.
    rdflib_memory = make_rdflib_memory_from(copy_graph)
    oxigraph_graph = make_oxigraph_from(copy_graph)

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
            mean, stddev, result = bench(lambda g=graph, fn=wfn: fn(g), repeat=repeat)
            print(fmt_row(bname, mean, stddev, result))
            rows.append(("row", wname, bname, (mean, stddev), result))
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
                (mean, stddev), _result = entry
                cells.append(f"{mean*1000:.2f} ± {stddev*1000:.2f} ms")
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

    try:
        from ontoenv import is_debug_build
        debug = bool(is_debug_build())
    except Exception:
        debug = False
    if debug:
        bar = "=" * 78
        print(
            f"\n{bar}\n"
            "WARNING: ontoenv was built in DEBUG (unoptimized) mode.\n"
            "Backends that cross the FFI per triple (ontoenv-get's ViewGraph)\n"
            "run 5-10x slower; pure-Python backends (ontoenv-copy, rdflib-memory)\n"
            "are unaffected, so the comparison is meaningless. Rebuild with\n"
            "  uv run maturin develop --release\n"
            f"before trusting these numbers.\n{bar}\n",
            file=sys.stderr,
        )

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
