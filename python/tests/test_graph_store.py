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


TTL_DEMO = "\n".join(
    [
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .",
        "<http://example.com/demo> a owl:Ontology .",
        "<http://example.com/demo> <http://example.com/p> \"v\" .",
    ]
)

TTL_IMPORTS = "\n".join(
    [
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .",
        "<http://example.com/base> a owl:Ontology .",
        "<http://example.com/base> owl:imports <http://example.com/demo> .",
    ]
)


class TestPythonGraphStore(unittest.TestCase):
    def test_python_graph_store_add_get(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            ttl_path = Path(td) / "demo.ttl"
            ttl_path.write_text(TTL_DEMO)

            store = DictGraphStore()
            env = OntoEnv(graph_store=store, temporary=True)
            iri = env.add(str(ttl_path))
            self.assertIn(iri, store.graphs)

            g = env.get_graph(iri)
            self.assertEqual(len(g), len(store.graphs[iri]))

    def test_init_from_store_loads_existing_graphs(self) -> None:
        """OntoEnv(init_from_store=True) reconstructs its state from graphs already in the store."""
        from rdflib import Graph as RdflibGraph, URIRef
        from rdflib.namespace import OWL, RDF

        # Pre-populate the store with a graph that declares an ontology.
        store = DictGraphStore()
        g = RdflibGraph()
        iri = "http://example.com/preloaded"
        g.add((URIRef(iri), RDF.type, OWL.Ontology))
        g.add((URIRef(iri), URIRef("http://example.com/p"), URIRef("http://example.com/o")))
        store.add_graph(iri, g)

        # Creating OntoEnv with init_from_store=True should pick up the pre-existing graph.
        env = OntoEnv(graph_store=store, temporary=True, init_from_store=True)
        names = env.get_ontology_names()
        self.assertIn(iri, names, f"expected {iri} in {names}")

    def test_init_from_store_empty_store(self) -> None:
        """OntoEnv(init_from_store=True) on an empty store starts with zero ontologies."""
        store = DictGraphStore()
        env = OntoEnv(graph_store=store, temporary=True, init_from_store=True)
        self.assertEqual(env.get_ontology_names(), [])

    def test_refresh_from_store_reflects_external_changes(self) -> None:
        """refresh_from_store() re-syncs the environment after external store mutations."""
        from rdflib import Graph as RdflibGraph, URIRef
        from rdflib.namespace import OWL, RDF

        store = DictGraphStore()
        env = OntoEnv(graph_store=store, temporary=True, init_from_store=True)

        # Environment starts empty.
        self.assertEqual(env.get_ontology_names(), [])

        # Add a graph to the store externally (bypassing OntoEnv).
        g = RdflibGraph()
        iri = "http://example.com/external"
        g.add((URIRef(iri), RDF.type, OWL.Ontology))
        store.add_graph(iri, g)

        # Before refresh, OntoEnv should not know about it.
        self.assertNotIn(iri, env.get_ontology_names())

        # After refresh, OntoEnv should reflect the store.
        env.refresh_from_store()
        names_after = env.get_ontology_names()
        self.assertIn(iri, names_after, f"expected {iri} in {names_after}")

    def test_refresh_from_store_removes_deleted_graphs(self) -> None:
        """refresh_from_store() removes ontologies whose graphs were deleted from the store."""
        from rdflib import Graph as RdflibGraph, URIRef
        from rdflib.namespace import OWL, RDF

        store = DictGraphStore()
        g = RdflibGraph()
        iri = "http://example.com/todelete"
        g.add((URIRef(iri), RDF.type, OWL.Ontology))
        store.add_graph(iri, g)

        env = OntoEnv(graph_store=store, temporary=True, init_from_store=True)
        self.assertIn(iri, env.get_ontology_names())

        # Remove the graph from the store externally.
        store.remove_graph(iri)
        env.refresh_from_store()

        self.assertNotIn(iri, env.get_ontology_names())


if __name__ == "__main__":
    unittest.main()
