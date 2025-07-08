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
        """Test env.add() with a local file."""
        self.env = OntoEnv(path=self.test_dir)
        name = self.env.add(str(self.brick_file_path))
        self.assertEqual(name, self.brick_name)
        ontologies = self.env.get_ontology_names()
        self.assertIn(self.brick_name, ontologies)

    def test_add_url(self):
        """Test env.add() with a URL."""
        cfg = Config(offline=False)
        self.env = OntoEnv(cfg, path=self.test_dir)
        name = self.env.add(self.brick_144_url)
        self.assertEqual(name, self.brick_144_name)
        ontologies = self.env.get_ontology_names()
        self.assertIn(self.brick_144_name, ontologies)

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
        g, _ = self.env.import_dependencies(g)
        num_triples_after = len(g)

        self.assertGreater(num_triples_after, num_triples_before)

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


if __name__ == "__main__":
    unittest.main()
