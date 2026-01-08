import tempfile
import unittest
from pathlib import Path

from rdflib import Graph

from ontoenv import OntoEnv


class DictGraphStore:
    def __init__(self) -> None:
        self.graphs: dict[str, Graph] = {}

    def add_graph(self, iri: str, graph: Graph, overwrite: bool = False) -> None:
        if not overwrite and iri in self.graphs:
            return
        self.graphs[iri] = graph

    def get_graph(self, iri: str) -> Graph:
        return self.graphs[iri]

    def remove_graph(self, iri: str) -> None:
        del self.graphs[iri]

    def graph_ids(self) -> list[str]:
        return list(self.graphs.keys())

    def size(self) -> dict[str, int]:
        return {
            "num_graphs": len(self.graphs),
            "num_triples": sum(len(g) for g in self.graphs.values()),
        }


class TestPythonGraphStore(unittest.TestCase):
    def test_python_graph_store_add_get(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            ttl_path = Path(td) / "demo.ttl"
            ttl_path.write_text(
                "\n".join(
                    [
                        "@prefix owl: <http://www.w3.org/2002/07/owl#> .",
                        "<http://example.com/demo> a owl:Ontology .",
                        "<http://example.com/demo> <http://example.com/p> \"v\" .",
                    ]
                )
            )

            store = DictGraphStore()
            env = OntoEnv(graph_store=store, temporary=True)
            iri = env.add(str(ttl_path))
            self.assertIn(iri, store.graphs)

            g = env.get_graph(iri)
            self.assertEqual(len(g), len(store.graphs[iri]))


if __name__ == "__main__":
    unittest.main()
