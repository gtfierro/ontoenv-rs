import unittest
import shutil
import os
from pathlib import Path
from ontoenv import OntoEnv
from rdflib import Graph, URIRef
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

    def test_add_rejects_in_memory_rdflib_graph(self):
        """Adding an rdflib.Graph object should raise since it is in-memory."""
        self.env = OntoEnv(temporary=True)
        g = Graph()
        ontology = URIRef("http://example.com/temp")
        g.add((ontology, RDF.type, OWL.Ontology))

        with self.assertRaises(TypeError) as ctx:
            self.env.add(g)
        self.assertIn("In-memory rdflib graphs cannot be added", str(ctx.exception))

        with self.assertRaises(TypeError):
            self.env.add_no_imports(g)

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

    def test_list_closure(self):
        """Test env.list_closure()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, search_directories=["brick"])
        name = self.env.add(str(self.brick_file_path))
        closure_list = self.env.list_closure(name)
        self.assertIn(name, closure_list)
        # check for some known imports
        self.assertIn("http://qudt.org/3.1.8/schema/qudt", closure_list)
        self.assertIn("http://qudt.org/3.1.8/vocab/quantitykind", closure_list)

    def test_get_importers(self):
        """Test env.get_importers()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, search_directories=["brick"])
        self.env.add(str(self.brick_file_path))

        dependents = self.env.get_importers("http://qudt.org/3.1.8/vocab/quantitykind")
        self.assertIn(self.brick_name, dependents)

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

    def test_to_rdflib_dataset(self):
        """Test env.to_rdflib_dataset()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, search_directories=["brick"])
        self.env.add(str(self.brick_file_path))
        self.env.update()  # need to run update to find all dependencies
        self.env.flush()

        ds = self.env.to_rdflib_dataset()
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

    def test_get_dependencies_graph(self):
        """Test env.get_dependencies_graph()."""
        self.env = OntoEnv(path=self.test_dir, recreate=True, offline=False)
        self.env.add(str(self.brick_file_path))

        g = Graph()
        brick_ontology_uri = URIRef(self.brick_name)
        g.add((brick_ontology_uri, RDF.type, OWL.Ontology))
        # add an import to be resolved
        g.add((brick_ontology_uri, OWL.imports, URIRef("http://qudt.org/3.1.8/vocab/quantitykind")))

        num_triples_before = len(g)
        deps_g, imported = self.env.get_dependencies_graph(g)
        num_triples_after = len(g)

        # original graph should not be modified
        self.assertEqual(num_triples_before, num_triples_after)

        # new graph should have content
        self.assertGreater(len(deps_g), 0)
        self.assertGreater(len(imported), 0)
        self.assertIn("http://qudt.org/3.1.8/vocab/quantitykind", imported)
        self.assertIn("http://qudt.org/3.1.8/vocab/dimensionvector", imported)

        # test with destination graph
        dest_g = Graph()
        self.assertEqual(len(dest_g), 0)
        deps_g2, imported2 = self.env.get_dependencies_graph(g, destination_graph=dest_g)

        # check that the returned graph is the same object as the destination graph
        self.assertIs(deps_g2, dest_g)
        self.assertGreater(len(dest_g), 0)
        self.assertEqual(len(deps_g), len(dest_g))
        self.assertEqual(sorted(imported), sorted(imported2))

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



if __name__ == "__main__":
    unittest.main()
