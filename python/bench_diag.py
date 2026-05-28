"""Diagnostic: where does the time in `ontoenv-get` actually go?"""
from __future__ import annotations
import gc, time
from contextlib import contextmanager
from rdflib import URIRef
from rdflib.namespace import OWL
from ontoenv import OntoEnv

BRICK = "/Users/gabe/src/ontoenv-rs/brick/Brick.ttl"
BRICK_IRI = "https://brickschema.org/schema/1.4/Brick"


@contextmanager
def timed(label):
    gc.collect()
    t0 = time.perf_counter()
    yield
    dt = time.perf_counter() - t0
    print(f"  {label:<60s} {dt*1000:10.2f} ms")


def main():
    env = OntoEnv(path=".bench-env", recreate=True, offline=False, strict=False)
    env.add(BRICK)
    env.update()
    env.flush()

    view, names = env.get_closure(BRICK_IRI)
    dataset = env.get_dataset()
    print(f"closure graphs: {len(names)}")
    print(f"dataset backend: {dataset.store._backend.backend_kind()}")

    print("\n--- iterate all triples ---")
    with timed("ClosureGraphView.triples((None,None,None))  [w/ dedup]"):
        n = sum(1 for _ in view.triples((None, None, None)))
        print(f"    n={n}")

    with timed("iter_closure_triples(BRICK_IRI)               [no dedup]"):
        n = sum(1 for _ in env.iter_closure_triples(BRICK_IRI))
        print(f"    n={n}")

    with timed("dataset.store.triples((None,None,None), None) [Rust scan]"):
        n = 0
        for _trip, _ctxs in dataset.store.triples((None, None, None), None):
            n += 1
        print(f"    n={n}")

    # Per-graph triples on the underlying dataset (what ClosureGraphView dispatches to)
    with timed("sum of dataset.graph(id).triples per graph    [no Python dedup]"):
        n = 0
        for ident in [URIRef(x) for x in names]:
            n += sum(1 for _ in dataset.graph(ident).triples((None, None, None)))
        print(f"    n={n}")

    print("\n--- pattern: ?s owl:imports ?o ---")
    with timed("ClosureGraphView.triples((None,OWL.imports,None))"):
        n = sum(1 for _ in view.triples((None, OWL.imports, None)))
        print(f"    n={n}")

    with timed("dataset.store.triples((None,OWL.imports,None), None)"):
        n = 0
        for _trip, _ctxs in dataset.store.triples((None, OWL.imports, None), None):
            n += 1
        print(f"    n={n}")

    print("\n--- SPARQL subClassOf* ---")
    Q = """
    PREFIX brick: <https://brickschema.org/schema/Brick#>
    SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE {
      ?s <http://www.w3.org/2000/01/rdf-schema#subClassOf>* brick:Equipment .
    }
    """
    with timed("view.query(...)        [via ClosureGraphView]"):
        rows = list(view.query(Q))
        print(f"    rows={rows}")

    with timed("dataset.query(...)     [direct on dataset]"):
        rows = list(dataset.query(Q))
        print(f"    rows={rows}")

    # Now constrain SPARQL to GRAPH <brick> only — closure-merged via UNION of GRAPH
    Q_GRAPHED = """
    PREFIX brick: <https://brickschema.org/schema/Brick#>
    SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE {
      GRAPH ?g {
        ?s <http://www.w3.org/2000/01/rdf-schema#subClassOf>* brick:Equipment .
      }
    }
    """
    with timed("dataset.query(...) with GRAPH ?g"):
        rows = list(dataset.query(Q_GRAPHED))
        print(f"    rows={rows}")

    env.close()


if __name__ == "__main__":
    main()
