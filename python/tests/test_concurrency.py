import unittest
import shutil
import multiprocessing
from pathlib import Path
from ontoenv import OntoEnv


def _ro_open_worker(path_str, graph_uri, result_queue):
    try:
        from pathlib import Path
        from ontoenv import OntoEnv
        from rdflib import URIRef
        from rdflib.namespace import RDF, OWL
        import time

        # Open the same store in read-only mode and load a specific graph + metadata
        env = OntoEnv(path=Path(path_str), read_only=True)
        g = env.get_graph(graph_uri)
        ok_graph = (URIRef(graph_uri), RDF.type, OWL.Ontology) in g and len(g) > 0
        ont = env.get_ontology(graph_uri)
        ok_meta = (ont.id == graph_uri)

        # Brief sleep to increase overlap
        time.sleep(0.2)
        env.close()
        result_queue.put(("ok", graph_uri) if (ok_graph and ok_meta) else ("missing", graph_uri))
    except Exception as e:
        result_queue.put(("error", graph_uri, str(e)))


def _rw_open_worker(path_str, graph_uri, result_queue):
    try:
        from pathlib import Path
        from ontoenv import OntoEnv
        from rdflib import URIRef
        from rdflib.namespace import RDF, OWL
        import time

        # Open the same store in read-write mode and load a specific graph + metadata
        env = OntoEnv(path=Path(path_str))
        g = env.get_graph(graph_uri)
        ok_graph = (URIRef(graph_uri), RDF.type, OWL.Ontology) in g and len(g) > 0
        ont = env.get_ontology(graph_uri)
        ok_meta = (ont.id == graph_uri)

        time.sleep(1.0)
        env.close()
        result_queue.put(("ok", graph_uri) if (ok_graph and ok_meta) else ("missing", graph_uri))
    except Exception as e:
        result_queue.put(("error", graph_uri, str(e)))


def _writer_hold_worker(path_str, hold_secs, graph_uri, result_queue):
    try:
        import time
        from pathlib import Path
        from ontoenv import OntoEnv
        from rdflib import URIRef
        from rdflib.namespace import RDF, OWL

        env = OntoEnv(path=Path(path_str))
        # Touch a known graph to ensure the store is usable
        g = env.get_graph(graph_uri)
        ok_graph = (URIRef(graph_uri), RDF.type, OWL.Ontology) in g and len(g) > 0

        # Hold the exclusive writer lock for a bit
        time.sleep(hold_secs)
        env.close()
        result_queue.put(("released", ok_graph))
    except Exception as e:
        result_queue.put(("error", str(e)))


def _ro_open_get_graph_worker(path_str, graph_uri, result_queue):
    try:
        from pathlib import Path
        from ontoenv import OntoEnv
        from rdflib import URIRef
        from rdflib.namespace import RDF, OWL

        env = OntoEnv(path=Path(path_str), read_only=True)
        g = env.get_graph(graph_uri)
        ok = (URIRef(graph_uri), RDF.type, OWL.Ontology) in g and len(g) > 0
        env.close()
        result_queue.put(("ok", ok))
    except Exception as e:
        result_queue.put(("error", str(e)))

class TestOntoEnvReadOnlyConcurrency(unittest.TestCase):
    def setUp(self):
        self.test_dir = Path("test_env_ro")
        if self.test_dir.exists():
            shutil.rmtree(self.test_dir)
        self.test_dir.mkdir()
        if Path(".ontoenv").exists():
            shutil.rmtree(".ontoenv")

    def tearDown(self):
        if self.test_dir.exists():
            shutil.rmtree(self.test_dir)
        if Path(".ontoenv").exists():
            shutil.rmtree(".ontoenv")

    def test_concurrent_read_only_open_same_store(self):
        # Pre-create a persistent store with two different ontologies
        a_path = self.test_dir / "A.ttl"
        b_path = self.test_dir / "B.ttl"
        a_uri = "http://example.org/ont/A"
        b_uri = "http://example.org/ont/B"
        a_path.write_text(
            "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n"
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
            f"<{a_uri}> a owl:Ontology .\n"
            f"<{a_uri}#Class1> a owl:Class .\n",
            encoding="utf-8",
        )
        b_path.write_text(
            "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n"
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
            f"<{b_uri}> a owl:Ontology .\n"
            f"<{b_uri}#Class2> a owl:Class .\n",
            encoding="utf-8",
        )

        # Create the store and add ontologies (single writer)
        env = OntoEnv(path=self.test_dir, recreate=True)
        name_a = env.add(str(a_path), fetch_imports=False)
        name_b = env.add(str(b_path), fetch_imports=False)
        self.assertEqual(name_a, a_uri)
        self.assertEqual(name_b, b_uri)
        env.flush()
        env.close()

        # Now, concurrently open the same store as read-only from two processes
        ctx = multiprocessing.get_context("spawn")
        q = ctx.Queue()
        store_path = str(self.test_dir.resolve())
        p1 = ctx.Process(target=_ro_open_worker, args=(store_path, name_a, q))
        p2 = ctx.Process(target=_ro_open_worker, args=(store_path, name_b, q))

        p1.start()
        p2.start()

        r1 = q.get(timeout=30)
        r2 = q.get(timeout=30)

        p1.join(timeout=30)
        p2.join(timeout=30)

        self.assertFalse(p1.is_alive())
        self.assertFalse(p2.is_alive())
        self.assertEqual(p1.exitcode, 0)
        self.assertEqual(p2.exitcode, 0)

        results = {r1, r2}
        self.assertIn(("ok", name_a), results)
        self.assertIn(("ok", name_b), results)


class TestOntoEnvRWConcurrency(unittest.TestCase):
    def setUp(self):
        self.test_dir = Path("test_env_py")
        if self.test_dir.exists():
            shutil.rmtree(self.test_dir)
        self.test_dir.mkdir()
        if Path(".ontoenv").exists():
            shutil.rmtree(".ontoenv")

    def tearDown(self):
        if self.test_dir.exists():
            shutil.rmtree(self.test_dir)
        if Path(".ontoenv").exists():
            shutil.rmtree(".ontoenv")

    def test_concurrent_open_same_store(self):
        """Attempt to open the same ontoenv store in two different processes, each loading a different graph."""
        # Pre-create a persistent store with two different ontologies
        a_path = self.test_dir / "A.ttl"
        b_path = self.test_dir / "B.ttl"
        a_uri = "http://example.org/ont/A"
        b_uri = "http://example.org/ont/B"
        a_path.write_text(
            "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n"
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
            f"<{a_uri}> a owl:Ontology .\n"
            f"<{a_uri}#Class1> a owl:Class .\n",
            encoding="utf-8",
        )
        b_path.write_text(
            "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n"
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
            f"<{b_uri}> a owl:Ontology .\n"
            f"<{b_uri}#Class2> a owl:Class .\n",
            encoding="utf-8",
        )

        env = OntoEnv(path=self.test_dir, recreate=True)
        name_a = env.add(str(a_path), fetch_imports=False)
        name_b = env.add(str(b_path), fetch_imports=False)
        self.assertEqual(name_a, a_uri)
        self.assertEqual(name_b, b_uri)
        env.flush()
        env.close()

        ctx = multiprocessing.get_context("spawn")
        q = ctx.Queue()
        p1 = ctx.Process(target=_rw_open_worker, args=(str(self.test_dir), name_a, q))
        p2 = ctx.Process(target=_rw_open_worker, args=(str(self.test_dir), name_b, q))

        p1.start()
        p2.start()

        r1 = q.get(timeout=30)
        r2 = q.get(timeout=30)

        p1.join(timeout=30)
        p2.join(timeout=30)

        # Verify both processes finished; one should succeed and one should report a lock error
        self.assertFalse(p1.is_alive())
        self.assertFalse(p2.is_alive())
        self.assertEqual(p1.exitcode, 0)
        self.assertEqual(p2.exitcode, 0)

        results = [r1, r2]
        ok_results = [r for r in results if r[0] == "ok"]
        error_results = [r for r in results if r[0] == "error"]

        # At least one process should fail to acquire the exclusive lock
        self.assertGreaterEqual(len(ok_results), 1)
        self.assertGreaterEqual(len(error_results), 1)

        # The successful open(s) should be for one of the graphs we added
        for tag, uri in ok_results:
            self.assertIn(uri, {name_a, name_b})

        # The error should mention failure to acquire exclusive lock for write
        err_msg = error_results[0][2]
        self.assertTrue(
            "Failed to open OntoEnv store for write" in err_msg or "exclusive lock" in err_msg,
            msg=f"Unexpected error message: {err_msg}",
        )

    def test_reader_waits_for_writer_then_reads(self):
        """A read-only open should wait while a writer holds the exclusive lock, then succeed."""
        a_path = self.test_dir / "A.ttl"
        a_uri = "http://example.org/ont/A"
        a_path.write_text(
            "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n"
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
            f"<{a_uri}> a owl:Ontology .\n",
            encoding="utf-8",
        )
        env = OntoEnv(path=self.test_dir, recreate=True)
        env.add(str(a_path), fetch_imports=False)
        env.flush()
        env.close()

        ctx = multiprocessing.get_context("spawn")
        q = ctx.Queue()
        hold_secs = 1.0
        writer = ctx.Process(target=_writer_hold_worker, args=(str(self.test_dir), hold_secs, a_uri, q))
        reader = ctx.Process(target=_ro_open_get_graph_worker, args=(str(self.test_dir), a_uri, q))

        writer.start()
        import time
        time.sleep(0.15)  # ensure writer started and holds the lock
        t0 = time.time()
        reader.start()

        # First result should be from reader after writer releases
        r1 = q.get(timeout=30)
        r2 = q.get(timeout=30)

        writer.join(timeout=30)
        reader.join(timeout=30)

        self.assertFalse(writer.is_alive())
        self.assertFalse(reader.is_alive())
        self.assertEqual(writer.exitcode, 0)
        self.assertEqual(reader.exitcode, 0)

        elapsed = time.time() - t0
        # Reader should have waited roughly the hold duration (minus start skew)
        self.assertGreaterEqual(elapsed, 0.7)

        results = {r1[0], r2[0]}
        self.assertIn("released", results)
        self.assertIn("ok", results)


if __name__ == "__main__":
    unittest.main()
