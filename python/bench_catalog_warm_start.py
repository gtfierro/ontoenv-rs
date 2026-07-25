"""Compare catalog warm starts for small and large custom-store graphs.

Run with:

    uv run python bench_catalog_warm_start.py

The timed section opens and closes an already-adopted environment. The store's
``get_graph`` counter verifies that graph size is not part of the warm path.
"""

from pathlib import Path
from tempfile import TemporaryDirectory
from time import perf_counter

from rdflib import Graph, Literal, URIRef
from rdflib.namespace import OWL, RDF

from ontoenv import OntoEnv


class BenchmarkStore:
    def __init__(self, graph: Graph, iri: str) -> None:
        self.graph = graph
        self.iri = iri
        self.get_calls = 0

    def add_graph(self, iri: str, graph: Graph, overwrite: bool = False) -> None:
        self.iri = iri
        self.graph = graph

    def get_graph(self, iri: str) -> Graph:
        assert iri == self.iri
        self.get_calls += 1
        return self.graph

    def remove_graph(self, iri: str) -> None:
        raise NotImplementedError

    def graph_ids(self) -> list[str]:
        return [self.iri]

    def store_state(self) -> dict[str, str]:
        return {"id": f"benchmark:{self.iri}", "revision": "1"}

    def graph_revisions(self) -> dict[str, str]:
        return {self.iri: "1"}


def make_graph(iri: str, triples: int) -> Graph:
    graph = Graph()
    subject = URIRef(iri)
    graph.add((subject, RDF.type, OWL.Ontology))
    predicate = URIRef("https://example.org/value")
    for index in range(max(0, triples - 1)):
        graph.add((URIRef(f"{iri}/resource/{index}"), predicate, Literal(index)))
    return graph


def measure(triples: int, iterations: int = 20) -> tuple[float, int]:
    iri = f"https://example.org/benchmark/{triples}"
    store = BenchmarkStore(make_graph(iri, triples), iri)
    with TemporaryDirectory() as directory:
        root = Path(directory)
        adopted = OntoEnv.adopt(root, store)
        adopted.close()
        calls_after_adoption = store.get_calls

        started = perf_counter()
        for _ in range(iterations):
            environment = OntoEnv.open(root, graph_store=store)
            environment.close()
        elapsed = perf_counter() - started
        return elapsed / iterations, store.get_calls - calls_after_adoption


if __name__ == "__main__":
    for triple_count in (1, 124_000):
        seconds, graph_reads = measure(triple_count)
        print(
            f"{triple_count:>7,} triples: {seconds * 1_000:8.3f} ms/open, "
            f"get_graph calls during warm opens: {graph_reads}"
        )
