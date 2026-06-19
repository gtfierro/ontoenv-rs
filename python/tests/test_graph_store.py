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


class TestTransientGraphQueries(unittest.TestCase):
    """Tests for list_closure / missing_imports on graphs not yet in the environment."""

    def _make_env_with_two_ontologies(self) -> tuple:
        """
        Returns (env, base_iri, dep_iri) where:
          - dep  = <dep_iri>  a owl:Ontology .   (no imports)
          - base = <base_iri> a owl:Ontology ; owl:imports <dep_iri> .
        Both are in the environment.
        """
        from rdflib import Graph as G, URIRef
        from rdflib.namespace import OWL, RDF

        dep_iri = "http://example.com/dep"
        base_iri = "http://example.com/base"

        store = DictGraphStore()

        dep_g = G()
        dep_g.add((URIRef(dep_iri), RDF.type, OWL.Ontology))
        store.add_graph(dep_iri, dep_g)

        base_g = G()
        base_g.add((URIRef(base_iri), RDF.type, OWL.Ontology))
        base_g.add((URIRef(base_iri), OWL.imports, URIRef(dep_iri)))
        store.add_graph(base_iri, base_g)

        env = OntoEnv(graph_store=store, temporary=True, init_from_store=True)
        return env, base_iri, dep_iri

    # ---- list_closure ----

    def test_list_closure_string_uri_unchanged(self) -> None:
        """list_closure(uri_str) still works as before."""
        env, base_iri, dep_iri = self._make_env_with_two_ontologies()
        closure = env.list_closure(base_iri)
        self.assertIn(base_iri, closure)
        self.assertIn(dep_iri, closure)

    def test_list_closure_transient_graph_returns_import_closure(self) -> None:
        """list_closure(graph) resolves the closure via the graph's owl:imports."""
        from rdflib import Graph as G, URIRef
        from rdflib.namespace import OWL, RDF

        env, base_iri, dep_iri = self._make_env_with_two_ontologies()

        # A brand-new graph that imports base (which is in the env).
        new_iri = "http://example.com/new"
        transient = G()
        transient.add((URIRef(new_iri), RDF.type, OWL.Ontology))
        transient.add((URIRef(new_iri), OWL.imports, URIRef(base_iri)))

        closure = env.list_closure(transient)
        # new graph leads the list, base and dep follow via the env.
        self.assertEqual(closure[0], new_iri)
        self.assertIn(base_iri, closure)
        self.assertIn(dep_iri, closure)

    def test_list_closure_transient_graph_unresolvable_import_omitted(self) -> None:
        """Imports that can't be found in the env are simply absent from the closure."""
        from rdflib import Graph as G, URIRef
        from rdflib.namespace import OWL, RDF

        env, _, _ = self._make_env_with_two_ontologies()

        ghost_iri = "http://example.com/ghost"
        transient = G()
        transient.add((URIRef("http://example.com/new2"), RDF.type, OWL.Ontology))
        transient.add((URIRef("http://example.com/new2"), OWL.imports, URIRef(ghost_iri)))

        closure = env.list_closure(transient)
        self.assertNotIn(ghost_iri, closure)

    def test_list_closure_wrong_type_raises(self) -> None:
        env, _, _ = self._make_env_with_two_ontologies()
        with self.assertRaises(TypeError):
            env.list_closure(12345)

    # ---- missing_imports ----

    def test_missing_imports_string_uri_unchanged(self) -> None:
        """missing_imports(uri_str) still works as before (no missing here)."""
        env, base_iri, _ = self._make_env_with_two_ontologies()
        self.assertEqual(env.missing_imports(base_iri), [])

    def test_missing_imports_transient_graph_all_present(self) -> None:
        """missing_imports(graph) returns [] when all imports are in the env."""
        from rdflib import Graph as G, URIRef
        from rdflib.namespace import OWL, RDF

        env, base_iri, _ = self._make_env_with_two_ontologies()

        transient = G()
        transient.add((URIRef("http://example.com/top"), RDF.type, OWL.Ontology))
        transient.add((URIRef("http://example.com/top"), OWL.imports, URIRef(base_iri)))

        self.assertEqual(env.missing_imports(transient), [])

    def test_missing_imports_transient_graph_direct_missing(self) -> None:
        """missing_imports(graph) reports imports not found in the env."""
        from rdflib import Graph as G, URIRef
        from rdflib.namespace import OWL, RDF

        env, _, _ = self._make_env_with_two_ontologies()

        ghost_iri = "http://example.com/ghost"
        transient = G()
        transient.add((URIRef("http://example.com/top"), RDF.type, OWL.Ontology))
        transient.add((URIRef("http://example.com/top"), OWL.imports, URIRef(ghost_iri)))

        missing = env.missing_imports(transient)
        self.assertIn(ghost_iri, missing)

    def test_missing_imports_transient_graph_transitive_missing(self) -> None:
        """missing_imports(graph) also catches missing imports inside the env's closure."""
        from rdflib import Graph as G, URIRef
        from rdflib.namespace import OWL, RDF

        env, base_iri, dep_iri = self._make_env_with_two_ontologies()

        # Add an ontology to the env that itself imports something missing.
        ghost_iri = "http://example.com/ghost"
        mid_iri = "http://example.com/mid"
        store_ext = DictGraphStore()
        mid_g = G()
        mid_g.add((URIRef(mid_iri), RDF.type, OWL.Ontology))
        mid_g.add((URIRef(mid_iri), OWL.imports, URIRef(ghost_iri)))
        store_ext.add_graph(mid_iri, mid_g)
        env2 = OntoEnv(graph_store=store_ext, temporary=True, init_from_store=True)

        # transient imports mid (which imports ghost, which is absent)
        transient = G()
        transient.add((URIRef("http://example.com/top"), RDF.type, OWL.Ontology))
        transient.add((URIRef("http://example.com/top"), OWL.imports, URIRef(mid_iri)))
        missing = env2.missing_imports(transient)
        self.assertIn(ghost_iri, missing)

    def test_missing_imports_wrong_type_raises(self) -> None:
        env, _, _ = self._make_env_with_two_ontologies()
        with self.assertRaises(TypeError):
            env.missing_imports(42)


class TestPythonGraphStoreReadParity(unittest.TestCase):
    """Custom graph_store= read parity for the copy/iter aggregation APIs.

    get_graph()/get_closure()/copy_graph() already route through the
    overridden GraphIO::get_graph and so work against a custom store. These
    tests pin down that the aggregating reads -- copy_closure, copy_union,
    iter_closure_triples, iter_triples -- reach the same parity instead of
    silently reading an empty backing store.
    """

    P = "http://example.com/p"
    DEP = "http://example.com/dep"
    BASE = "http://example.com/base"

    def _env(self):
        from rdflib import Graph as G, Literal, URIRef
        from rdflib.namespace import OWL, RDF

        store = DictGraphStore()

        dep_g = G()
        dep_g.add((URIRef(self.DEP), RDF.type, OWL.Ontology))
        dep_g.add((URIRef(self.DEP), URIRef(self.P), Literal("dep-value")))
        store.add_graph(self.DEP, dep_g)

        base_g = G()
        base_g.add((URIRef(self.BASE), RDF.type, OWL.Ontology))
        base_g.add((URIRef(self.BASE), OWL.imports, URIRef(self.DEP)))
        base_g.add((URIRef(self.BASE), URIRef(self.P), Literal("base-value")))
        store.add_graph(self.BASE, base_g)

        return OntoEnv(graph_store=store, temporary=True, init_from_store=True)

    def _content_values(self, graph):
        from rdflib import URIRef

        return {str(o) for _, _, o in graph.triples((None, URIRef(self.P), None))}

    def test_copy_graph_parity(self) -> None:
        env = self._env()
        g = env.copy_graph(self.DEP)
        self.assertEqual(self._content_values(g), {"dep-value"})

    def test_copy_closure_includes_imported_graphs(self) -> None:
        env = self._env()
        g, names = env.copy_closure(self.BASE)
        self.assertEqual(self._content_values(g), {"base-value", "dep-value"})
        self.assertIn(self.BASE, names)
        self.assertIn(self.DEP, names)

    def test_copy_closure_into_destination_graph(self) -> None:
        from rdflib import Graph as G

        env = self._env()
        dest = G()
        g, _ = env.copy_closure(self.BASE, graph=dest)
        self.assertIs(g, dest)
        self.assertEqual(self._content_values(dest), {"base-value", "dep-value"})

    def test_copy_closure_remove_owl_imports(self) -> None:
        from rdflib import URIRef
        from rdflib.namespace import OWL

        env = self._env()
        kept, _ = env.copy_closure(self.BASE, remove_owl_imports=False)
        self.assertIn(
            (URIRef(self.BASE), OWL.imports, URIRef(self.DEP)),
            kept,
        )
        dropped, _ = env.copy_closure(self.BASE, remove_owl_imports=True)
        self.assertNotIn(
            (URIRef(self.BASE), OWL.imports, URIRef(self.DEP)),
            dropped,
        )

    def test_copy_union_includes_listed_graphs(self) -> None:
        env = self._env()
        g, names = env.copy_union([self.BASE, self.DEP], root=self.BASE)
        self.assertEqual(self._content_values(g), {"base-value", "dep-value"})
        self.assertIn(self.BASE, names)
        self.assertIn(self.DEP, names)

    def test_iter_triples_parity(self) -> None:
        env = self._env()
        values = {
            str(o)
            for (_, p, o) in env.iter_triples(self.BASE)
            if str(p) == self.P
        }
        self.assertEqual(values, {"base-value"})

    def test_iter_closure_triples_parity(self) -> None:
        env = self._env()
        values = {
            str(o)
            for (_, p, o) in env.iter_closure_triples(self.BASE)
            if str(p) == self.P
        }
        self.assertEqual(values, {"base-value", "dep-value"})


if __name__ == "__main__":
    unittest.main()
