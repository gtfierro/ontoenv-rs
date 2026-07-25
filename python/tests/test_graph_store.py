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


def temporary_env_from_store(store: DictGraphStore) -> OntoEnv:
    """Build a catalog-free environment and deliberately scan the test store."""
    env = OntoEnv(graph_store=store, temporary=True)
    env.refresh_from_store(full=True)
    return env


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
        with self.assertWarnsRegex(DeprecationWarning, "init_from_store is deprecated"):
            env = OntoEnv(graph_store=store, temporary=True, init_from_store=True)
        names = env.get_ontology_names()
        self.assertIn(iri, names, f"expected {iri} in {names}")

    def test_init_from_store_empty_store(self) -> None:
        """OntoEnv(init_from_store=True) on an empty store starts with zero ontologies."""
        store = DictGraphStore()
        with self.assertWarnsRegex(DeprecationWarning, "init_from_store is deprecated"):
            env = OntoEnv(graph_store=store, temporary=True, init_from_store=True)
        self.assertEqual(env.get_ontology_names(), [])

    def test_refresh_from_store_reflects_external_changes(self) -> None:
        """refresh_from_store() re-syncs the environment after external store mutations."""
        from rdflib import Graph as RdflibGraph, URIRef
        from rdflib.namespace import OWL, RDF

        store = DictGraphStore()
        env = temporary_env_from_store(store)

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

        env = temporary_env_from_store(store)
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

        env = temporary_env_from_store(store)
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
        env2 = temporary_env_from_store(store_ext)

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

        return temporary_env_from_store(store)

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

    def test_copy_dataset_parity(self) -> None:
        from rdflib import URIRef

        env = self._env()
        ds = env.copy_dataset()
        base_g = ds.graph(URIRef(self.BASE))
        dep_g = ds.graph(URIRef(self.DEP))
        self.assertEqual(self._content_values(base_g), {"base-value"})
        self.assertEqual(self._content_values(dep_g), {"dep-value"})

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


class CopyCapableStore(DictGraphStore):
    """DictGraphStore that also implements copy_graph.

    copy_graph adds a marker triple so tests can prove the copy path was
    dispatched rather than the get path.
    """

    MARKER_P = "http://example.com/copy-marker"

    def copy_graph(self, iri: str) -> Graph:
        from rdflib import Literal, URIRef

        g = Graph()
        g += self.graphs[iri]
        g.add((URIRef(iri), URIRef(self.MARKER_P), Literal(True)))
        return g


def _triples(graph) -> set:
    return {(str(s), str(p), str(o)) for s, p, o in graph}


class TestCopyGetParity(unittest.TestCase):
    """copy_* and get_* methods return the same content with no copy_graph override.

    Uses DictGraphStore (no copy_graph method) so copy operations fall back to
    get_graph. The test pinpoints that every copy method returns content
    identical to its get counterpart — no extra or missing triples.
    """

    P = "http://example.com/p"
    DEP = "http://example.com/dep"
    BASE = "http://example.com/base"

    def _env(self) -> "OntoEnv":
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

        return temporary_env_from_store(store)

    def test_copy_graph_matches_get_graph(self) -> None:
        env = self._env()
        self.assertEqual(_triples(env.copy_graph(self.DEP)), _triples(env.get_graph(self.DEP)))
        self.assertEqual(_triples(env.copy_graph(self.BASE)), _triples(env.get_graph(self.BASE)))

    def test_copy_closure_matches_get_closure(self) -> None:
        env = self._env()
        # Disable optional post-processing so copy_closure is as close to the
        # raw view as possible. copy_closure always calls remove_ontology_declarations
        # (not controllable via a flag), so the rdf:type owl:Ontology triple
        # for non-root members is absent from the copy but present in the view.
        # Compare only application triples (predicate = self.P) to avoid that
        # structural difference while still proving the copy content is correct.
        copy_g, copy_names = env.copy_closure(
            self.BASE, remove_owl_imports=False, rewrite_sh_prefixes=False
        )
        get_g, get_names = env.get_closure(self.BASE)
        copy_content = {t for t in _triples(copy_g) if t[1] == self.P}
        get_content = {t for t in _triples(get_g) if t[1] == self.P}
        self.assertEqual(copy_content, get_content)
        self.assertEqual(set(copy_names), set(get_names))

    def test_copy_union_matches_get_union(self) -> None:
        env = self._env()
        # Same comparison strategy as copy_closure / get_closure: compare only
        # application triples because copy_union normalises ontology declarations.
        copy_g, copy_names = env.copy_union(
            [self.BASE, self.DEP],
            root=self.BASE,
            remove_owl_imports=False,
            rewrite_sh_prefixes=False,
        )
        get_g, get_names = env.get_union([self.BASE, self.DEP])
        copy_content = {t for t in _triples(copy_g) if t[1] == self.P}
        get_content = {t for t in _triples(get_g) if t[1] == self.P}
        self.assertEqual(copy_content, get_content)
        self.assertEqual(set(copy_names), set(get_names))

    def test_get_union_with_closures(self) -> None:
        env = self._env()
        # With include_closures=True, listing only BASE should pull in DEP too.
        get_g, names = env.get_union([self.BASE], include_closures=True)
        content = {str(o) for _, p, o in get_g if str(p) == self.P}
        self.assertEqual(content, {"base-value", "dep-value"})
        self.assertIn(self.BASE, names)
        self.assertIn(self.DEP, names)

    def test_copy_dataset_matches_get_dataset(self) -> None:
        from rdflib import URIRef

        env = self._env()
        copy_ds = env.copy_dataset()
        get_ds = env.get_dataset()

        for uri in [self.BASE, self.DEP]:
            copy_triples = _triples(copy_ds.graph(URIRef(uri)))
            get_triples = _triples(get_ds.graph(URIRef(uri)))
            self.assertEqual(copy_triples, get_triples, f"mismatch for {uri}")


class TestCopyDispatch(unittest.TestCase):
    """copy_* dispatches to the store's copy_graph when available.

    Uses CopyCapableStore, whose copy_graph adds a marker triple. Tests
    check that the marker appears in copy results and is absent from get
    results, proving copy and get dispatch to different store methods.
    """

    DEP = "http://example.com/dep"
    BASE = "http://example.com/base"
    P = "http://example.com/p"
    MARKER_P = CopyCapableStore.MARKER_P

    def _env(self) -> "OntoEnv":
        from rdflib import Graph as G, Literal, URIRef
        from rdflib.namespace import OWL, RDF

        store = CopyCapableStore()

        dep_g = G()
        dep_g.add((URIRef(self.DEP), RDF.type, OWL.Ontology))
        dep_g.add((URIRef(self.DEP), URIRef(self.P), Literal("dep-value")))
        store.add_graph(self.DEP, dep_g)

        base_g = G()
        base_g.add((URIRef(self.BASE), RDF.type, OWL.Ontology))
        base_g.add((URIRef(self.BASE), OWL.imports, URIRef(self.DEP)))
        base_g.add((URIRef(self.BASE), URIRef(self.P), Literal("base-value")))
        store.add_graph(self.BASE, base_g)

        return temporary_env_from_store(store)

    def _has_marker(self, graph, subject_iri: str) -> bool:
        from rdflib import Literal, URIRef

        return (URIRef(subject_iri), URIRef(self.MARKER_P), Literal(True)) in graph

    def test_copy_graph_dispatches_to_store_copy_graph(self) -> None:
        env = self._env()
        copy_g = env.copy_graph(self.DEP)
        self.assertTrue(self._has_marker(copy_g, self.DEP))

    def test_get_graph_does_not_use_copy_graph(self) -> None:
        env = self._env()
        get_g = env.get_graph(self.DEP)
        self.assertFalse(self._has_marker(get_g, self.DEP))

    def test_copy_closure_dispatches_to_store_copy_graph(self) -> None:
        env = self._env()
        copy_g, _ = env.copy_closure(self.BASE)
        self.assertTrue(self._has_marker(copy_g, self.BASE))
        self.assertTrue(self._has_marker(copy_g, self.DEP))

    def test_get_closure_does_not_use_copy_graph(self) -> None:
        env = self._env()
        get_g, _ = env.get_closure(self.BASE)
        self.assertFalse(self._has_marker(get_g, self.BASE))
        self.assertFalse(self._has_marker(get_g, self.DEP))

    def test_copy_union_dispatches_to_store_copy_graph(self) -> None:
        env = self._env()
        copy_g, _ = env.copy_union([self.BASE, self.DEP], root=self.BASE)
        self.assertTrue(self._has_marker(copy_g, self.BASE))
        self.assertTrue(self._has_marker(copy_g, self.DEP))

    def test_get_union_does_not_use_copy_graph(self) -> None:
        env = self._env()
        get_g, _ = env.get_union([self.BASE, self.DEP])
        self.assertFalse(self._has_marker(get_g, self.BASE))
        self.assertFalse(self._has_marker(get_g, self.DEP))

    def test_copy_dataset_dispatches_to_store_copy_graph(self) -> None:
        from rdflib import URIRef

        env = self._env()
        ds = env.copy_dataset()
        self.assertTrue(self._has_marker(ds.graph(URIRef(self.DEP)), self.DEP))
        self.assertTrue(self._has_marker(ds.graph(URIRef(self.BASE)), self.BASE))

    def test_get_dataset_does_not_use_copy_graph(self) -> None:
        from rdflib import URIRef

        env = self._env()
        ds = env.get_dataset()
        self.assertFalse(self._has_marker(ds.graph(URIRef(self.DEP)), self.DEP))
        self.assertFalse(self._has_marker(ds.graph(URIRef(self.BASE)), self.BASE))


if __name__ == "__main__":
    unittest.main()
