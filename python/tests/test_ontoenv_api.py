import unittest
import shutil
import os
from pathlib import Path
from ontoenv import OntoEnv
from rdflib import Graph, URIRef, Literal
from rdflib.namespace import RDF, OWL, SH


class TestOntoEnvAPI(unittest.TestCase):
    def setUp(self):
        """Set up a test environment."""
        self.test_dir = Path("test_env_py")
        if self.test_dir.exists():
            shutil.rmtree(self.test_dir)
        self.test_dir.mkdir()

        self.brick_file_path = Path("../brick/Brick.ttl")
        self.brick_name = "https://brickschema.org/schema/1.4/Brick"
        self.brick_144_url = "https://brickschema.org/schema/1.4.4/Brick.ttl"
        self.brick_144_name = "https://brickschema.org/schema/1.4/Brick"
        self.env = None

        # clean up any existing env in current dir
        if Path(".ontoenv").exists():
            shutil.rmtree(".ontoenv")

    def tearDown(self):
        """Tear down the test environment."""
        if self.env:
            self.env.close()
        if self.test_dir.exists():
            shutil.rmtree(self.test_dir)
        if Path(".ontoenv").exists():
            shutil.rmtree(".ontoenv")

    def test_constructor_default(self):
        """Test default OntoEnv() constructor respects git-style discovery."""
        original_cwd = Path.cwd()
        os.chdir(self.test_dir)
        try:
            bootstrap = OntoEnv(create_or_use_cached=True)
            bootstrap.close()
            self.env = OntoEnv()
            self.assertIn("OntoEnv", repr(self.env))
        finally:
            os.chdir(original_cwd)
        
    def test_constructor_path(self):
        """Test OntoEnv(path=...) constructor."""
        self.env = OntoEnv(path=self.test_dir, recreate=True)
        self.assertTrue((self.test_dir / ".ontoenv").is_dir())

    def test_constructor_with_config(self):
        """Test OntoEnv(...flags...) constructor."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, search_directories=["../brick"])
        self.env.update()  # discover ontologies
        ontologies = self.env.get_ontology_names()
        self.assertIn(self.brick_name, ontologies)

    def test_add_local_file(self):
        """Test env.add() with a local file and fetching imports."""
        # requires offline=False to fetch QUDT from web
        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=False)
        name = self.env.add(str(self.brick_file_path))
        self.assertEqual(name, self.brick_name)
        ontologies = self.env.get_ontology_names()
        self.assertIn(self.brick_name, ontologies)
        # check that dependencies were added because fetch_imports is true by default
        self.assertIn("http://qudt.org/3.1.8/schema/qudt", ontologies)

    def test_add_url(self):
        """Test env.add() with a URL."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=False)
        name = self.env.add(self.brick_144_url)
        self.assertEqual(name, self.brick_144_name)
        ontologies = self.env.get_ontology_names()
        self.assertIn(self.brick_144_name, ontologies)
        # check that dependencies were added because fetch_imports is true by default
        self.assertIn("http://qudt.org/3.1.0/schema/qudt", ontologies)

    def test_add_no_fetch_imports(self):
        """Test env.add() with fetch_imports=False."""
        self.env = OntoEnv(path=self.test_dir, recreate=True)
        # With fetch_imports=False, Brick should be added but its dependencies
        # should not be processed.
        name = self.env.add(str(self.brick_file_path), fetch_imports=False)
        self.assertEqual(name, self.brick_name)
        ontologies = self.env.get_ontology_names()
        self.assertIn(self.brick_name, ontologies)
        # check that dependencies were not added
        self.assertEqual(len(ontologies), 1)

    def test_add_in_memory_rdflib_graph_resolves_imports_and_dependency_edges(self):
        """In-memory rdflib.Graph add should resolve imports and update dependency edges."""
        dep_path = self.test_dir / "dep.ttl"
        dep_iri = dep_path.resolve().as_uri()
        dep_path.write_text(
            f"""
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            <{dep_iri}> a owl:Ontology .
            """.strip()
            + "\n",
            encoding="utf-8",
        )

        root_iri = "http://example.com/root-in-memory"
        g = Graph()
        root = URIRef(root_iri)
        g.add((root, RDF.type, OWL.Ontology))
        g.add((root, OWL.imports, URIRef(dep_iri)))
        g.add((URIRef(f"{root_iri}#s"), URIRef("http://example.com/p"), Literal("v")))

        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=True)
        added_name = self.env.add(g)
        self.assertEqual(added_name, root_iri)

        names = self.env.get_ontology_names()
        self.assertIn(root_iri, names)
        self.assertIn(dep_iri, names)

        closure = self.env.list_closure(root_iri)
        self.assertIn(root_iri, closure)
        self.assertIn(dep_iri, closure)

        importers = self.env.get_importers(dep_iri)
        self.assertIn(root_iri, importers)

    def test_add_in_memory_rdflib_graph_matches_file_backed_closure(self):
        """In-memory rdflib add should match file-backed add closure for same ontology content."""
        dep_path = self.test_dir / "dep.ttl"
        dep_iri = dep_path.resolve().as_uri()
        dep_path.write_text(
            f"""
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            <{dep_iri}> a owl:Ontology .
            """.strip()
            + "\n",
            encoding="utf-8",
        )

        root_iri = "http://example.com/root-parity"
        g = Graph()
        root = URIRef(root_iri)
        g.add((root, RDF.type, OWL.Ontology))
        g.add((root, OWL.imports, URIRef(dep_iri)))
        g.add((URIRef(f"{root_iri}#s"), URIRef("http://example.com/p"), Literal("v")))
        root_ttl = g.serialize(format="turtle")

        root_path = self.test_dir / "root.ttl"
        root_path.write_text(root_ttl, encoding="utf-8")

        env_graph_path = self.test_dir / "env_graph"
        env_file_path = self.test_dir / "env_file"
        env_graph_path.mkdir()
        env_file_path.mkdir()

        env_graph = OntoEnv(path=env_graph_path, recreate=True, offline=True)
        env_file = OntoEnv(path=env_file_path, recreate=True, offline=True)
        try:
            in_memory_root = env_graph.add(g)
            file_root = env_file.add(str(root_path))

            closure_graph = sorted(env_graph.list_closure(in_memory_root))
            closure_file = sorted(env_file.list_closure(file_root))
            self.assertEqual(closure_graph, closure_file)
        finally:
            env_graph.close()
            env_file.close()

    def test_add_in_memory_rdflib_graph_strict_errors_on_missing_import(self):
        """Strict mode should error when an in-memory graph imports a missing ontology."""
        missing_iri = (self.test_dir / "missing.ttl").resolve().as_uri()
        root_iri = "http://example.com/root-strict-missing"

        g = Graph()
        root = URIRef(root_iri)
        g.add((root, RDF.type, OWL.Ontology))
        g.add((root, OWL.imports, URIRef(missing_iri)))

        self.env = OntoEnv(path=self.test_dir, recreate=True, strict=True, offline=True)
        with self.assertRaises(ValueError):
            self.env.add(g)

        self.assertEqual(self.env.get_ontology_names(), [])

    def test_add_in_memory_rdflib_graph_non_strict_skips_missing_import(self):
        """Non-strict mode should keep the root when an in-memory import is missing."""
        missing_iri = (self.test_dir / "missing.ttl").resolve().as_uri()
        root_iri = "http://example.com/root-non-strict-missing"

        g = Graph()
        root = URIRef(root_iri)
        g.add((root, RDF.type, OWL.Ontology))
        g.add((root, OWL.imports, URIRef(missing_iri)))

        self.env = OntoEnv(path=self.test_dir, recreate=True, strict=False, offline=True)
        added_name = self.env.add(g)
        self.assertEqual(added_name, root_iri)

        names = self.env.get_ontology_names()
        self.assertIn(root_iri, names)
        self.assertNotIn(missing_iri, names)

        closure = self.env.list_closure(root_iri)
        self.assertEqual(len(closure), 1)
        self.assertEqual(closure[0], root_iri)

    def test_get_closure_with_in_memory_destination(self):
        """Closure can be materialized into an in-memory rdflib.Graph."""
        base_path = self.test_dir / "base.ttl"
        imported_path = self.test_dir / "imported.ttl"
        imported_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex: <http://example.com/imported#> .
            <http://example.com/imported> a owl:Ontology .
            ex:Thing a owl:Class .
            """.strip()
            + "\n",
            encoding="utf-8",
        )
        base_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex: <http://example.com/base#> .
            <http://example.com/base> a owl:Ontology ;
                owl:imports <http://example.com/imported> .
            ex:Root a owl:Class .
            """.strip()
            + "\n",
            encoding="utf-8",
        )

        self.env = OntoEnv(path=self.test_dir, recreate=True)
        # Load imported first so fetch_imports finds it locally.
        self.env.add(str(imported_path), fetch_imports=False)
        base_name = self.env.add(str(base_path))

        destination = Graph()
        closure_graph, closure_names = self.env.get_closure(base_name, destination_graph=destination)

        self.assertIs(destination, closure_graph)
        self.assertGreater(len(closure_graph), 0)
        self.assertIn("http://example.com/base", closure_names)
        self.assertIn("http://example.com/imported", closure_names)

    def test_get_graph(self):
        """Test env.get_graph()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True)
        name = self.env.add(str(self.brick_file_path))
        g = self.env.get_graph(name)
        self.assertIsInstance(g, Graph)
        self.assertGreater(len(g), 0)
        self.assertIn((URIRef(self.brick_name), RDF.type, OWL.Ontology), g)
        with self.assertRaises(ValueError):
            g.add((URIRef("urn:test"), RDF.type, OWL.Ontology))

        materialized = self.env.copy_graph(name)
        self.assertIsInstance(materialized, Graph)
        materialized.add((URIRef("urn:test"), RDF.type, OWL.Ontology))
        self.assertIn((URIRef("urn:test"), RDF.type, OWL.Ontology), materialized)

    def test_get_closure_view(self):
        """get_closure_view returns a read-only merged view + name list."""
        from ontoenv import ClosureGraphView

        self.env = OntoEnv(
            path=self.test_dir, recreate=True, search_directories=["brick"]
        )
        name = self.env.add(str(self.brick_file_path))

        view, names = self.env.get_closure_view(name, recursion_depth=0)

        self.assertIsInstance(view, ClosureGraphView)
        self.assertEqual(names[0], name)
        self.assertGreater(len(view), 0)
        self.assertIn((URIRef(name), RDF.type, OWL.Ontology), view)

        # Same shape as get_closure for trivial swap-ability.
        closure_g, closure_names = self.env.get_closure(name, recursion_depth=0)
        self.assertEqual(set(names), set(closure_names))

        # Read-only.
        with self.assertRaises(ValueError):
            view.add((URIRef("urn:x"), RDF.type, OWL.Ontology))

    def test_iter_triples_and_iter_closure_triples(self):
        """Streaming iterators yield rdflib-term tuples without an rdflib.Graph."""
        self.env = OntoEnv(
            path=self.test_dir, recreate=True, search_directories=["brick"]
        )
        name = self.env.add(str(self.brick_file_path))

        single = list(self.env.iter_triples(name))
        self.assertGreater(len(single), 0)
        s, p, o = single[0]
        self.assertTrue(hasattr(s, "n3"))  # rdflib Identifier

        # Closure stream should include at least as many triples as the
        # single-graph stream (root is always in the closure).
        closure = list(self.env.iter_closure_triples(name, recursion_depth=0))
        self.assertGreaterEqual(len(closure), len(single))

    def test_get_graph_caches_dataset_across_calls(self):
        """Repeated get_graph calls reuse the same underlying Dataset/Store."""
        self.env = OntoEnv(path=self.test_dir, recreate=True)
        name = self.env.add(str(self.brick_file_path))

        first = self.env.get_graph(name)
        second = self.env.get_graph(name)
        self.assertIs(first.store, second.store)

        # A mutating call invalidates the cache, so the next view is built
        # against a fresh store.
        self.env.flush()
        after_flush = self.env.get_graph(name)
        self.assertIsNot(after_flush.store, first.store)

    def test_dunder_sugar(self):
        """len(env), uri in env, env[uri], iter(env), and context manager."""
        with OntoEnv(path=self.test_dir, recreate=True) as env:
            name = env.add(str(self.brick_file_path))

            self.assertGreater(len(env), 0)
            self.assertIn(name, env)
            self.assertNotIn("urn:does-not-exist", env)
            self.assertFalse("not a uri" in env)

            self.assertIn(name, list(iter(env)))

            graph_via_getitem = env[name]
            self.assertIsInstance(graph_via_getitem, Graph)
            self.assertGreater(len(graph_via_getitem), 0)

        # context manager exit calls close(); subsequent calls raise
        with self.assertRaises(ValueError):
            len(env)

    def test_get_closure(self):
        """Test env.get_closure()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, search_directories=["brick"])
        name = self.env.add(str(self.brick_file_path))
        g = self.env.get_graph(name)
        closure_g, imported_graphs = self.env.get_closure(name, recursion_depth=0)
        self.assertIsInstance(closure_g, Graph)
        self.assertEqual(len(imported_graphs), 1)

        closure_g, imported_graphs = self.env.get_closure(name)
        self.assertIsInstance(closure_g, Graph)
        self.assertGreater(len(imported_graphs), 1)
        self.assertGreater(len(closure_g), len(g))

    def test_import_dependencies(self):
        """Test env.import_dependencies()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, search_directories=["brick"])
        self.env.add(str(self.brick_file_path))

        g = Graph()
        brick_ontology_uri = URIRef(self.brick_name)
        g.add((brick_ontology_uri, RDF.type, OWL.Ontology))
        # add an import to be removed
        g.add((brick_ontology_uri, OWL.imports, URIRef("http://qudt.org/3.1.8/schema/qudt")))

        num_triples_before = len(g)
        imported = self.env.import_dependencies(g)
        self.assertGreater(len(imported), 0)
        num_triples_after = len(g)

        self.assertGreater(num_triples_after, num_triples_before)

    def test_import_dependencies_fetch_missing(self):
        """Test env.import_dependencies() with fetch_missing=True."""
        # offline=False is required to fetch from URL
        # empty env
        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=False)
        
        g = Graph()
        # Add an import to a known ontology URL that is not in the environment
        g.add(
            (
                URIRef("http://example.org/my-ontology"),
                OWL.imports,
                URIRef(self.brick_144_url),
            )
        )

        num_triples_before = len(g)
        # With fetch_missing=True, this should download Brick and its dependencies
        imported = self.env.import_dependencies(g, fetch_missing=True)
        self.assertGreater(len(imported), 0)
        self.assertIn(self.brick_144_name, imported)
        num_triples_after = len(g)

        self.assertGreater(num_triples_after, num_triples_before)

        # check that the fetched ontologies are now in the environment
        ontologies = self.env.get_ontology_names()
        self.assertIn(self.brick_144_name, ontologies)

    def test_import_dependencies_rewrites_sh_prefixes_to_root(self):
        """sh:prefixes should be rewritten onto the root ontology after import_dependencies."""
        self.env = OntoEnv(path=self.test_dir, recreate=True)

        a_path = self.test_dir / "A.ttl"
        b_path = self.test_dir / "B.ttl"
        c_path = self.test_dir / "C.ttl"

        a_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://ex.org/> .
            <http://ex.org/A> a owl:Ontology ;
              owl:imports <http://ex.org/B> .
            ex:shape sh:prefixes <http://ex.org/A> .
            """.strip()
            + "\n",
            encoding="utf-8",
        )
        b_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix exb: <http://ex.org/b#> .
            <http://ex.org/B> a owl:Ontology ;
              owl:imports <http://ex.org/C> .
            exb:shape sh:prefixes <http://ex.org/B> .
            """.strip()
            + "\n",
            encoding="utf-8",
        )
        c_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix exc: <http://ex.org/c#> .
            <http://ex.org/C> a owl:Ontology .
            exc:shape sh:prefixes <http://ex.org/C> .
            """.strip()
            + "\n",
            encoding="utf-8",
        )

        self.env.add(str(a_path), fetch_imports=False)
        self.env.add(str(b_path), fetch_imports=False)
        self.env.add(str(c_path), fetch_imports=False)

        g = Graph()
        root = URIRef("http://ex.org/A")
        g.add((root, RDF.type, OWL.Ontology))
        g.add((root, OWL.imports, URIRef("http://ex.org/B")))

        self.env.import_dependencies(g)

        prefixes = list(g.triples((None, SH.prefixes, None)))
        self.assertGreater(len(prefixes), 0)
        self.assertTrue(all(o == root for _, _, o in prefixes), prefixes)

    def test_import_dependencies_rewrites_sh_prefixes_large_union(self):
        """sh:prefixes should all point to root in a larger dependency graph."""
        self.env = OntoEnv(path=self.test_dir, recreate=True)

        a_path = self.test_dir / "A.ttl"
        b_path = self.test_dir / "B.ttl"
        c_path = self.test_dir / "C.ttl"
        d_path = self.test_dir / "D.ttl"
        e_path = self.test_dir / "E.ttl"
        f_path = self.test_dir / "F.ttl"

        a_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <http://ex.org/> .
            <http://ex.org/A> a owl:Ontology ;
              owl:imports <http://ex.org/B> ,
                          <http://ex.org/C> ,
                          <http://ex.org/D> ,
                          <http://ex.org/E> .
            ex:shape sh:prefixes <http://ex.org/A> .
            """.strip()
            + "\n",
            encoding="utf-8",
        )
        b_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix exb: <http://ex.org/b#> .
            <http://ex.org/B> a owl:Ontology .
            exb:shape sh:prefixes <http://ex.org/B> .
            """.strip()
            + "\n",
            encoding="utf-8",
        )
        c_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix exc: <http://ex.org/c#> .
            <http://ex.org/C> a owl:Ontology .
            exc:shape sh:prefixes <http://ex.org/C> .
            """.strip()
            + "\n",
            encoding="utf-8",
        )
        d_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix exd: <http://ex.org/d#> .
            <http://ex.org/D> a owl:Ontology .
            exd:shape sh:prefixes <http://ex.org/D> .
            """.strip()
            + "\n",
            encoding="utf-8",
        )
        e_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix exe: <http://ex.org/e#> .
            <http://ex.org/E> a owl:Ontology ;
              owl:imports <http://ex.org/F> .
            exe:shape sh:prefixes <http://ex.org/E> .
            """.strip()
            + "\n",
            encoding="utf-8",
        )
        f_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix exf: <http://ex.org/f#> .
            <http://ex.org/F> a owl:Ontology .
            exf:shape sh:prefixes <http://ex.org/F> .
            """.strip()
            + "\n",
            encoding="utf-8",
        )

        self.env.add(str(a_path))
        self.env.add(str(b_path))
        self.env.add(str(c_path))
        self.env.add(str(d_path))
        self.env.add(str(e_path))
        self.env.add(str(f_path))

        g = Graph()
        root = URIRef("http://ex.org/A")
        g.add((root, RDF.type, OWL.Ontology))
        g.add((root, OWL.imports, URIRef("http://ex.org/B")))
        g.add((root, OWL.imports, URIRef("http://ex.org/C")))
        g.add((root, OWL.imports, URIRef("http://ex.org/D")))
        g.add((root, OWL.imports, URIRef("http://ex.org/E")))

        self.env.import_dependencies(g)

        prefixes = list(g.triples((None, SH.prefixes, None)))
        self.assertGreater(len(prefixes), 0)
        self.assertTrue(all(o == root for _, _, o in prefixes), prefixes)

    def test_list_closure(self):
        """Test env.list_closure()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, search_directories=["brick"])
        name = self.env.add(str(self.brick_file_path))
        closure_list = self.env.list_closure(name)
        self.assertIn(name, closure_list)
        # check for some known imports
        self.assertIn("http://qudt.org/3.1.8/schema/qudt", closure_list)
        self.assertIn("http://qudt.org/3.1.8/vocab/quantitykind", closure_list)

    def test_list_closure_is_selective(self):
        """list_closure(A) must return only A's transitive imports, not unrelated ontologies."""
        # Build two independent import chains: A -> B and C -> D
        a_path = self.test_dir / "a.ttl"
        b_path = self.test_dir / "b.ttl"
        c_path = self.test_dir / "c.ttl"
        d_path = self.test_dir / "d.ttl"

        a_iri = a_path.resolve().as_uri()
        b_iri = b_path.resolve().as_uri()
        c_iri = c_path.resolve().as_uri()
        d_iri = d_path.resolve().as_uri()

        b_path.write_text(
            f"@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{b_iri}> a owl:Ontology .\n",
            encoding="utf-8",
        )
        a_path.write_text(
            f"@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
            f"<{a_iri}> a owl:Ontology ; owl:imports <{b_iri}> .\n",
            encoding="utf-8",
        )
        d_path.write_text(
            f"@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<{d_iri}> a owl:Ontology .\n",
            encoding="utf-8",
        )
        c_path.write_text(
            f"@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
            f"<{c_iri}> a owl:Ontology ; owl:imports <{d_iri}> .\n",
            encoding="utf-8",
        )

        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=True)
        self.env.add(str(b_path))
        self.env.add(str(a_path))
        self.env.add(str(d_path))
        self.env.add(str(c_path))

        closure_a = self.env.list_closure(a_iri)

        # A's closure must contain A and B
        self.assertIn(a_iri, closure_a)
        self.assertIn(b_iri, closure_a)

        # A's closure must NOT contain C or D (they are in a separate chain)
        self.assertNotIn(c_iri, closure_a)
        self.assertNotIn(d_iri, closure_a)

    def test_get_importers(self):
        """Test env.get_importers()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, search_directories=["brick"])
        self.env.add(str(self.brick_file_path))

        dependents = self.env.get_importers("http://qudt.org/3.1.8/vocab/quantitykind")
        self.assertIn(self.brick_name, dependents)

    def test_missing_imports_environment_level(self):
        """missing_imports() with no args returns IRIs of unresolvable imports across the env."""
        ont_path = self.test_dir.resolve() / "ont.ttl"
        ont_iri = ont_path.as_uri()
        missing_iri = "http://example.com/missing-ontology"

        ont_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{ont_iri}> a owl:Ontology ;
    owl:imports <{missing_iri}> .
""",
            encoding="utf-8",
        )

        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=True)
        self.env.add(str(ont_path), fetch_imports=False)

        missing = self.env.missing_imports()
        self.assertIn(missing_iri, missing)

    def test_missing_imports_per_graph(self):
        """missing_imports(uri) returns only the unresolvable imports for that ontology."""
        a_path = (self.test_dir / "a.ttl").resolve()
        b_path = (self.test_dir / "b.ttl").resolve()
        a_iri = a_path.as_uri()
        b_iri = b_path.as_uri()
        missing_iri = "http://example.com/not-here"

        b_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{b_iri}> a owl:Ontology .
""",
            encoding="utf-8",
        )
        a_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{a_iri}> a owl:Ontology ;
    owl:imports <{b_iri}> ;
    owl:imports <{missing_iri}> .
""",
            encoding="utf-8",
        )

        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=True)
        self.env.add(str(b_path))
        self.env.add(str(a_path), fetch_imports=False)

        missing = self.env.missing_imports(a_iri)
        self.assertIn(missing_iri, missing)
        self.assertNotIn(b_iri, missing)

    def test_missing_imports_per_graph_transitive(self):
        """missing_imports(uri) surfaces missing imports from the full transitive closure."""
        a_path = (self.test_dir / "a.ttl").resolve()
        b_path = (self.test_dir / "b.ttl").resolve()
        a_iri = a_path.as_uri()
        b_iri = b_path.as_uri()
        missing_iri = "http://example.com/transitively-missing"

        # B is present but declares an import that is not in the env.
        b_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{b_iri}> a owl:Ontology ;
    owl:imports <{missing_iri}> .
""",
            encoding="utf-8",
        )
        # A imports B (which is present); A has no missing direct imports.
        a_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{a_iri}> a owl:Ontology ;
    owl:imports <{b_iri}> .
""",
            encoding="utf-8",
        )

        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=True)
        self.env.add(str(b_path), fetch_imports=False)
        self.env.add(str(a_path), fetch_imports=False)

        # The env-level call should surface the transitively-missing IRI.
        self.assertIn(missing_iri, self.env.missing_imports())

        # The per-graph call starting from A should also find it, even though
        # it is declared by B (not A directly).
        missing = self.env.missing_imports(a_iri)
        self.assertIn(missing_iri, missing)
        self.assertNotIn(b_iri, missing)

    def test_missing_imports_three_level_chain(self):
        """A->B->C where C is not loaded: both missing_imports(A) and missing_imports(B) report C."""
        a_path = (self.test_dir / "a.ttl").resolve()
        b_path = (self.test_dir / "b.ttl").resolve()
        a_iri = a_path.as_uri()
        b_iri = b_path.as_uri()
        c_iri = "http://example.com/c-not-loaded"

        b_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{b_iri}> a owl:Ontology ;
    owl:imports <{c_iri}> .
""",
            encoding="utf-8",
        )
        a_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{a_iri}> a owl:Ontology ;
    owl:imports <{b_iri}> .
""",
            encoding="utf-8",
        )

        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=True)
        self.env.add(str(b_path), fetch_imports=False)
        self.env.add(str(a_path), fetch_imports=False)

        # B directly declares C as missing.
        self.assertIn(c_iri, self.env.missing_imports(b_iri))
        # A's closure includes B, which declares C — so A should also report C.
        self.assertIn(c_iri, self.env.missing_imports(a_iri))

    def test_missing_imports_empty_when_all_resolved(self):
        """missing_imports() returns empty list when all imports are resolvable."""
        a_path = (self.test_dir / "a.ttl").resolve()
        b_path = (self.test_dir / "b.ttl").resolve()
        a_iri = a_path.as_uri()
        b_iri = b_path.as_uri()

        b_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{b_iri}> a owl:Ontology .
""",
            encoding="utf-8",
        )
        a_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{a_iri}> a owl:Ontology ;
    owl:imports <{b_iri}> .
""",
            encoding="utf-8",
        )

        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=True)
        self.env.add(str(b_path))
        self.env.add(str(a_path))

        self.assertEqual(self.env.missing_imports(), [])
        self.assertEqual(self.env.missing_imports(a_iri), [])

    def test_import_graph_flattens_to_single_ontology(self):
        """import_graph merges closure into one ontology declaration and removes owl:imports."""
        base_path = self.test_dir / "base.ttl"
        imp_path = self.test_dir / "imp.ttl"
        base_iri = base_path.resolve().as_uri()
        imp_iri = imp_path.resolve().as_uri()

        imp_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix ex: <http://example.com/imp#> .
<{imp_iri}> a owl:Ontology .
ex:ImpClass a owl:Class .
""",
            encoding="utf-8",
        )
        base_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix ex: <http://example.com/base#> .
<{base_iri}> a owl:Ontology ;
    owl:imports <{imp_iri}> .
ex:BaseClass a owl:Class .
""",
            encoding="utf-8",
        )

        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=True)
        self.env.add(str(imp_path))
        self.env.add(str(base_path))

        dest = Graph()
        dest.add((URIRef(base_iri), RDF.type, OWL.Ontology))

        self.env.import_graph(dest, base_iri, recursion_depth=-1)

        # Only one ontology declaration (the root) should remain
        ontology_decls = list(dest.triples((None, RDF.type, OWL.Ontology)))
        self.assertEqual(len(ontology_decls), 1)
        self.assertEqual(str(ontology_decls[0][0]), base_iri)

        # Imports rewritten onto root, not to the imported ontology
        imports = list(dest.triples((URIRef(base_iri), OWL.imports, None)))
        self.assertEqual(len(imports), 1)
        self.assertEqual(str(imports[0][2]), imp_iri)

        # Data from both base and imported ontologies present
        self.assertTrue(any("BaseClass" in str(t) for t in dest))
        self.assertTrue(any("ImpClass" in str(t) for t in dest))

    def test_import_graph_handles_cycles(self):
        """import_graph should handle cycles (A imports B imports A) without duplicating imports."""
        a_path = self.test_dir / "A.ttl"
        b_path = self.test_dir / "B.ttl"
        a_iri = a_path.resolve().as_uri()
        b_iri = b_path.resolve().as_uri()

        a_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix ex: <http://example.com/A#> .
<{a_iri}> a owl:Ontology ;
    owl:imports <{b_iri}> .
ex:A a owl:Class .
""",
            encoding="utf-8",
        )
        b_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix ex: <http://example.com/B#> .
<{b_iri}> a owl:Ontology ;
    owl:imports <{a_iri}> .
ex:B a owl:Class .
""",
            encoding="utf-8",
        )

        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=True)
        self.env.add(str(a_path))
        self.env.add(str(b_path))

        dest = Graph()
        dest.add((URIRef(a_iri), RDF.type, OWL.Ontology))

        self.env.import_graph(dest, a_iri, recursion_depth=-1)

        # Single root ontology declaration
        ontology_decls = list(dest.triples((None, RDF.type, OWL.Ontology)))
        self.assertEqual(len(ontology_decls), 1)
        self.assertEqual(str(ontology_decls[0][0]), a_iri)

        # Imports rewritten onto root; no self-import duplication
        imports = list(dest.triples((URIRef(a_iri), OWL.imports, None)))
        self.assertEqual(len(imports), 1)
        self.assertEqual(str(imports[0][2]), b_iri)

        # No imports hanging off the imported ontology
        self.assertEqual(len(list(dest.triples((URIRef(b_iri), OWL.imports, None)))), 0)

        # Data from both ontologies present
        self.assertTrue(any("A" in str(t) for t in dest))
        self.assertTrue(any("B" in str(t) for t in dest))

    def test_import_graph_respects_recursion_depth(self):
        """import_graph should honor recursion_depth when reattaching imports."""
        a_path = self.test_dir / "A.ttl"
        b_path = self.test_dir / "B.ttl"
        c_path = self.test_dir / "C.ttl"
        a_iri = a_path.resolve().as_uri()
        b_iri = b_path.resolve().as_uri()
        c_iri = c_path.resolve().as_uri()

        a_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{a_iri}> a owl:Ontology ; owl:imports <{b_iri}> .
""",
            encoding="utf-8",
        )
        b_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{b_iri}> a owl:Ontology ; owl:imports <{c_iri}> .
""",
            encoding="utf-8",
        )
        c_path.write_text(
            f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<{c_iri}> a owl:Ontology .
""",
            encoding="utf-8",
        )

        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=True)
        self.env.add(str(a_path))
        self.env.add(str(b_path))
        self.env.add(str(c_path))

        dest0 = Graph()
        dest0.add((URIRef(a_iri), RDF.type, OWL.Ontology))
        self.env.import_graph(dest0, a_iri, recursion_depth=0)
        self.assertEqual(len(list(dest0.triples((URIRef(a_iri), OWL.imports, None)))), 0)

        dest1 = Graph()
        dest1.add((URIRef(a_iri), RDF.type, OWL.Ontology))
        self.env.import_graph(dest1, a_iri, recursion_depth=1)
        imports1 = list(dest1.triples((URIRef(a_iri), OWL.imports, None)))
        self.assertEqual(len(imports1), 1)
        self.assertEqual(str(imports1[0][2]), b_iri)

        dest_full = Graph()
        dest_full.add((URIRef(a_iri), RDF.type, OWL.Ontology))
        self.env.import_graph(dest_full, a_iri, recursion_depth=-1)
        imports_full = list(dest_full.triples((URIRef(a_iri), OWL.imports, None)))
        self.assertEqual(len(imports_full), 2)
        self.assertSetEqual(
            {str(i[2]) for i in imports_full},
            {b_iri, c_iri},
        )

    def test_as_dataset(self):
        """Test env.as_dataset()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, search_directories=["brick"])
        self.env.add(str(self.brick_file_path))
        self.env.update()  # need to run update to find all dependencies
        self.env.flush()

        ds = self.env.as_dataset()
        # count graphs
        num_graphs = len(list(ds.graphs()))
        # there should be many graphs: brick + all imports
        self.assertGreater(num_graphs, 5)

    def test_import_graph(self):
        """Test env.import_graph()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=False)
        name = self.env.add(self.brick_144_url)
        self.assertEqual(name, self.brick_144_name)

        g = Graph()
        self.assertEqual(len(g), 0)
        # import full closure; ensure imports were materialized and owl:imports removed
        self.env.import_graph(g, name, recursion_depth=-1)
        self.assertGreater(len(g), 0)
        # owl:imports should be rewritten onto the root ontology
        imports_pred = URIRef("http://www.w3.org/2002/07/owl#imports")
        imports = list(g.triples((URIRef(name), imports_pred, None)))
        self.assertGreater(len(imports), 0)

    def test_store_path(self):
        """Test env.store_path()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True)
        path = self.env.store_path()
        self.assertIsNotNone(path)
        self.assertTrue(Path(path).is_dir())
        self.assertIn(".ontoenv", path)

        # for in-memory, it should be None
        mem_env = OntoEnv(temporary=True)
        self.assertIsNone(mem_env.store_path())
        mem_env.close()

    def test_persistence(self):
        """Test that the environment is persisted to disk."""
        env = OntoEnv(path=self.test_dir, recreate=True)
        name = env.add(str(self.brick_file_path))
        self.assertIn(name, env.get_ontology_names())
        env.flush()  # ensure everything is written to disk
        env.close()

        # load it again from the same path
        self.env = OntoEnv(path=self.test_dir)
        self.assertIn(name, self.env.get_ontology_names())
        g = self.env.get_graph(name)
        self.assertGreater(len(g), 0)

    def test_close(self):
        """Test that the environment can be closed and methods fail."""
        self.env = OntoEnv(path=self.test_dir, recreate=True)
        name = self.env.add(str(self.brick_file_path))
        self.assertIn(name, self.env.get_ontology_names())
        self.env.close()

        # check that methods raise a ValueError
        with self.assertRaises(ValueError):
            self.env.get_ontology_names()
        with self.assertRaises(ValueError):
            self.env.get_graph(name)
        with self.assertRaises(ValueError):
            self.env.add(str(self.brick_file_path))

        # check __repr__
        self.assertIn("closed", repr(self.env))

        # store path should be None
        self.assertIsNone(self.env.store_path())

        # closing again should be fine
        self.env.close()

        # check that we can still create a new env from the same directory,
        # which should load the persisted state.
        env2 = OntoEnv(path=self.test_dir)
        self.assertIn(name, env2.get_ontology_names())
        env2.close()

    def test_get_dependencies(self):
        """Test env.get_dependencies()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=False)
        self.env.add(str(self.brick_file_path))

        g = Graph()
        brick_ontology_uri = URIRef(self.brick_name)
        g.add((brick_ontology_uri, RDF.type, OWL.Ontology))
        # add an import to be resolved
        g.add((brick_ontology_uri, OWL.imports, URIRef("http://qudt.org/3.1.8/vocab/quantitykind")))

        num_triples_before = len(g)
        deps_g, imported = self.env.get_dependencies(g)
        num_triples_after = len(g)

        # original graph should not be modified
        self.assertEqual(num_triples_before, num_triples_after)

        # new graph should have content
        self.assertGreater(len(deps_g), 0)
        self.assertGreater(len(imported), 0)
        self.assertIn("http://qudt.org/3.1.8/vocab/quantitykind", imported)
        self.assertIn("http://qudt.org/3.1.8/vocab/dimensionvector", imported)

        # without graph_name: each closure ontology keeps its own owl:Ontology declaration;
        # the result is a proper union, not an ownerless bag.
        ontology_decls = list(deps_g.triples((None, RDF.type, OWL.Ontology)))
        self.assertGreater(len(ontology_decls), 0, "deps graph should retain individual owl:Ontology declarations")
        # each declared ontology should be one of the imported IRIs
        declared_iris = {str(s) for s, _, _ in ontology_decls}
        self.assertTrue(declared_iris.issubset(set(imported)), "all ontology declarations should be from the closure")

        # with graph_name: returned graph has exactly one declaration (for graph_name only),
        # with sh:prefixes rewritten onto it
        named_uri = "http://example.org/my-merged-deps"
        deps_named, _ = self.env.get_dependencies(g, graph_name=named_uri)
        all_named_decls = list(deps_named.triples((None, RDF.type, OWL.Ontology)))
        self.assertEqual(len(all_named_decls), 1, "named deps graph should have exactly one owl:Ontology declaration")
        self.assertEqual(str(all_named_decls[0][0]), named_uri, "the single declaration should be for graph_name")

    def test_update_all_flag(self):
        """Test env.update(all=True) forces reloading of all ontologies."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, search_directories=["../brick"])
        # Initial discovery of ontologies
        self.env.update()
        self.assertIn(self.brick_name, self.env.get_ontology_names())

        ont1 = self.env.get_ontology(self.brick_name)
        ts1 = ont1.last_updated
        self.assertIsNotNone(ts1)

        # Force update of all ontologies
        self.env.update(all=True)

        ont2 = self.env.get_ontology(self.brick_name)
        ts2 = ont2.last_updated
        self.assertIsNotNone(ts2)
        self.assertNotEqual(ts1, ts2)



    def test_import_dependencies_errors_on_conflicting_prefix(self):
        """import_dependencies raises ValueError when the input rdflib graph declares
        an sh:prefix that conflicts with one from a dependency ontology."""
        b_path = self.test_dir / "B.ttl"
        b_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            <http://ex.org/B> a owl:Ontology ;
              sh:declare [
                sh:prefix "ex" ;
                sh:namespace <http://example.com/ns/two#>
              ] .
            """.strip()
            + "\n",
            encoding="utf-8",
        )
        self.env = OntoEnv(path=self.test_dir, recreate=True)
        self.env.add(str(b_path))

        g = Graph()
        root = URIRef("http://ex.org/A")
        g.add((root, RDF.type, OWL.Ontology))
        g.add((root, OWL.imports, URIRef("http://ex.org/B")))
        # Add a conflicting prefix declaration on the root: same prefix "ex", different namespace
        from rdflib import BNode, Literal
        decl = BNode()
        g.add((root, SH.declare, decl))
        g.add((decl, SH.prefix, Literal("ex")))
        g.add((decl, SH.namespace, URIRef("http://example.com/ns/one#")))

        with self.assertRaises(ValueError) as ctx:
            self.env.import_dependencies(g)
        self.assertIn('Conflicting sh:prefix "ex"', str(ctx.exception))

    def test_get_closure_errors_on_conflicting_prefix(self):
        """get_closure raises ValueError when two stored ontologies have conflicting
        sh:prefix declarations and rewrite_sh_prefixes=True."""
        a_path = self.test_dir / "A.ttl"
        b_path = self.test_dir / "B.ttl"
        a_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            <http://ex.org/A> a owl:Ontology ;
              owl:imports <http://ex.org/B> ;
              sh:declare [
                sh:prefix "ex" ;
                sh:namespace <http://example.com/ns/one#>
              ] .
            """.strip()
            + "\n",
            encoding="utf-8",
        )
        b_path.write_text(
            """
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            <http://ex.org/B> a owl:Ontology ;
              sh:declare [
                sh:prefix "ex" ;
                sh:namespace <http://example.com/ns/two#>
              ] .
            """.strip()
            + "\n",
            encoding="utf-8",
        )
        self.env = OntoEnv(path=self.test_dir, recreate=True)
        self.env.add(str(a_path))
        self.env.add(str(b_path))

        with self.assertRaises(ValueError) as ctx:
            self.env.get_closure("http://ex.org/A", rewrite_sh_prefixes=True)
        self.assertIn('Conflicting sh:prefix "ex"', str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
