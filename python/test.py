import unittest
import shutil
from pathlib import Path
from ontoenv import OntoEnv, Config
from rdflib import Graph, URIRef
from rdflib.namespace import RDF, OWL


class TestOntoEnvAPI(unittest.TestCase):
    def setUp(self):
        """Set up a test environment."""
        self.test_dir = Path("test_env_py")
        if self.test_dir.exists():
            shutil.rmtree(self.test_dir)
        self.test_dir.mkdir()

        self.brick_file_path = Path("../brick/Brick.ttl")
        self.brick_name = "https://brickschema.org/schema/1.4-rc1/Brick"
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
        """Test default OntoEnv() constructor."""
        self.env = OntoEnv()
        self.assertTrue(Path(".ontoenv").is_dir())
        self.assertIn("ontologies", repr(self.env))

    def test_constructor_path(self):
        """Test OntoEnv(path=...) constructor."""
        self.env = OntoEnv(path=self.test_dir)
        self.assertTrue((self.test_dir / ".ontoenv").is_dir())

    def test_constructor_with_config(self):
        """Test OntoEnv(config=...) constructor."""
        cfg = Config(search_directories=["../brick"])
        self.env = OntoEnv(config=cfg, path=self.test_dir)
        self.env.update()  # discover ontologies
        ontologies = self.env.get_ontology_names()
        self.assertIn(self.brick_name, ontologies)

    def test_add_local_file(self):
        """Test env.add() with a local file and fetching imports."""
        # requires offline=False to fetch QUDT from web
        cfg = Config(offline=False)
        self.env = OntoEnv(config=cfg, path=self.test_dir)
        name = self.env.add(str(self.brick_file_path))
        self.assertEqual(name, self.brick_name)
        ontologies = self.env.get_ontology_names()
        self.assertIn(self.brick_name, ontologies)
        # check that dependencies were added because fetch_imports is true by default
        self.assertIn("http://qudt.org/2.1/schema/qudt", ontologies)

    def test_add_url(self):
        """Test env.add() with a URL."""
        cfg = Config(offline=False)
        self.env = OntoEnv(config=cfg, path=self.test_dir)
        name = self.env.add(self.brick_144_url)
        self.assertEqual(name, self.brick_144_name)
        ontologies = self.env.get_ontology_names()
        self.assertIn(self.brick_144_name, ontologies)
        # check that dependencies were added because fetch_imports is true by default
        self.assertIn("http://qudt.org/3.1.0/schema/qudt", ontologies)

    def test_add_no_fetch_imports(self):
        """Test env.add() with fetch_imports=False."""
        self.env = OntoEnv(path=self.test_dir)
        # With fetch_imports=False, Brick should be added but its dependencies
        # should not be processed.
        name = self.env.add(str(self.brick_file_path), fetch_imports=False)
        self.assertEqual(name, self.brick_name)
        ontologies = self.env.get_ontology_names()
        self.assertIn(self.brick_name, ontologies)
        # check that dependencies were not added
        self.assertEqual(len(ontologies), 1)

    def test_get_graph(self):
        """Test env.get_graph()."""
        self.env = OntoEnv(path=self.test_dir)
        name = self.env.add(str(self.brick_file_path))
        g = self.env.get_graph(name)
        self.assertIsInstance(g, Graph)
        self.assertGreater(len(g), 0)
        self.assertIn((URIRef(self.brick_name), RDF.type, OWL.Ontology), g)

    def test_get_closure(self):
        """Test env.get_closure()."""
        cfg = Config(search_directories=["brick"])
        self.env = OntoEnv(config=cfg, path=self.test_dir)
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
        cfg = Config(search_directories=["brick"])
        self.env = OntoEnv(config=cfg, path=self.test_dir)
        self.env.add(str(self.brick_file_path))

        g = Graph()
        brick_ontology_uri = URIRef(self.brick_name)
        g.add((brick_ontology_uri, RDF.type, OWL.Ontology))
        # add an import to be removed
        g.add((brick_ontology_uri, OWL.imports, URIRef("http://qudt.org/2.1/schema/qudt")))

        num_triples_before = len(g)
        imported = self.env.import_dependencies(g)
        self.assertGreater(len(imported), 0)
        num_triples_after = len(g)

        self.assertGreater(num_triples_after, num_triples_before)

    def test_import_dependencies_fetch_missing(self):
        """Test env.import_dependencies() with fetch_missing=True."""
        # offline=False is required to fetch from URL
        cfg = Config(offline=False)
        # empty env
        self.env = OntoEnv(config=cfg, path=self.test_dir)

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

    def test_list_closure(self):
        """Test env.list_closure()."""
        cfg = Config(search_directories=["brick"])
        self.env = OntoEnv(config=cfg, path=self.test_dir)
        name = self.env.add(str(self.brick_file_path))
        closure_list = self.env.list_closure(name)
        self.assertIn(name, closure_list)
        # check for some known imports
        self.assertIn("http://qudt.org/2.1/schema/qudt", closure_list)
        self.assertIn("http://qudt.org/2.1/vocab/quantitykind", closure_list)

    def test_get_importers(self):
        """Test env.get_importers()."""
        cfg = Config(search_directories=["brick"])
        self.env = OntoEnv(config=cfg, path=self.test_dir)
        self.env.add(str(self.brick_file_path))

        dependents = self.env.get_importers("http://qudt.org/2.1/vocab/quantitykind")
        self.assertIn(self.brick_name, dependents)

    def test_to_rdflib_dataset(self):
        """Test env.to_rdflib_dataset()."""
        cfg = Config(search_directories=["brick"])
        self.env = OntoEnv(config=cfg, path=self.test_dir)
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
        cfg = Config(offline=False)
        self.env = OntoEnv(config=cfg, path=self.test_dir)
        name = self.env.add(self.brick_144_url)
        self.assertEqual(name, self.brick_144_name)

        g = Graph()
        self.assertEqual(len(g), 0)
        self.env.import_graph(g, name)
        self.assertGreater(len(g), 0)

    def test_store_path(self):
        """Test env.store_path()."""
        self.env = OntoEnv(path=self.test_dir)
        path = self.env.store_path()
        self.assertIsNotNone(path)
        self.assertTrue(Path(path).is_dir())
        self.assertIn(".ontoenv", path)

        # for in-memory, it should be None
        cfg = Config(temporary=True)
        mem_env = OntoEnv(config=cfg)
        self.assertIsNone(mem_env.store_path())
        mem_env.close()

    def test_persistence(self):
        """Test that the environment is persisted to disk."""
        env = OntoEnv(path=self.test_dir)
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
        self.env = OntoEnv(path=self.test_dir)
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
        cfg = Config(offline=False)
        self.env = OntoEnv(config=cfg, path=self.test_dir)
        self.env.add(str(self.brick_file_path))

        g = Graph()
        brick_ontology_uri = URIRef(self.brick_name)
        g.add((brick_ontology_uri, RDF.type, OWL.Ontology))
        # add an import to be resolved
        g.add((brick_ontology_uri, OWL.imports, URIRef("http://qudt.org/2.1/vocab/quantitykind")))

        num_triples_before = len(g)
        deps_g, imported = self.env.get_dependencies_graph(g)
        num_triples_after = len(g)

        # original graph should not be modified
        self.assertEqual(num_triples_before, num_triples_after)

        # new graph should have content
        self.assertGreater(len(deps_g), 0)
        self.assertGreater(len(imported), 0)
        self.assertIn("http://qudt.org/2.1/vocab/quantitykind", imported)
        self.assertIn("http://qudt.org/2.1/vocab/dimensionvector", imported)

        # test with destination graph
        dest_g = Graph()
        self.assertEqual(len(dest_g), 0)
        deps_g2, imported2 = self.env.get_dependencies_graph(g, destination_graph=dest_g)

        # check that the returned graph is the same object as the destination graph
        self.assertIs(deps_g2, dest_g)
        self.assertGreater(len(dest_g), 0)
        self.assertEqual(len(deps_g), len(dest_g))
        self.assertEqual(sorted(imported), sorted(imported2))


if __name__ == "__main__":
    unittest.main()
