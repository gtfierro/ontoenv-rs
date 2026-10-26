import unittest
import shutil
import os
import tempfile
from pathlib import Path
from ontoenv import OntoEnv


class TestOntoEnvInit(unittest.TestCase):
    def tearDown(self):
        # Clean up any accidental .ontoenv in cwd
        if Path(".ontoenv-snapshots").exists():
            shutil.rmtree(".ontoenv-snapshots")

    def test_init_recreate_new_dir(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            env_path = root / "new_env"
            self.assertFalse(env_path.exists())
            env = OntoEnv(path=env_path, recreate=True)
            self.assertTrue((env_path / ".ontoenv-snapshots").is_dir())
            sp = env.store_path()
            self.assertIsNotNone(sp)
            self.assertTrue(Path(sp).is_dir())

    def test_init_recreate_existing_empty_dir(self):
        with tempfile.TemporaryDirectory() as td:
            env_path = Path(td) / "empty_env"
            env_path.mkdir()
            self.assertTrue(env_path.is_dir())
            env = OntoEnv(path=env_path, recreate=True)
            self.assertTrue((env_path / ".ontoenv-snapshots").is_dir())
            self.assertIsNotNone(env.store_path())

    def test_init_load_from_existing_dir(self):
        with tempfile.TemporaryDirectory() as td:
            env_path = Path(td) / "existing_env"
            env_path.mkdir()
            env = OntoEnv(path=env_path, recreate=True)
            env.flush()
            del env
            # load existing
            env2 = OntoEnv(path=env_path, read_only=False)
            self.assertTrue(
                Path(env2.store_path()).samefile(env_path / ".ontoenv-snapshots")
            )

    def test_init_recreate_existing_dir(self):
        with tempfile.TemporaryDirectory() as td:
            env_path = Path(td) / "existing_env"
            env_path.mkdir()
            env = OntoEnv(path=env_path, recreate=True)
            (env_path / ".ontoenv-snapshots" / "dummy.txt").touch()
            self.assertTrue((env_path / ".ontoenv-snapshots" / "dummy.txt").exists())
            payload = env_path / "payload.ttl"
            payload.write_text('<urn:reset> a <http://www.w3.org/2002/07/owl#Ontology> .')
            env.add(str(payload), fetch_imports=False)
            # Close the single writer before publishing the logical reset.
            old = env.snapshot()
            env.close()
            env = OntoEnv(path=env_path, recreate=True)
            self.assertGreater(env.snapshot().info["generation"], old.info["generation"])
            self.assertTrue((env_path / ".ontoenv-snapshots").is_dir())
            self.assertTrue((env_path / ".ontoenv-snapshots" / "dummy.txt").exists())
            self.assertEqual(len(env.get_ontology_names()), 0)
            self.assertEqual(len(old.view_graph("urn:reset")), 1)

    def test_init_read_only(self):
        with tempfile.TemporaryDirectory() as td:
            env_path = Path(td) / "existing_env"
            dummy_file = Path(td) / "dummy.ttl"
            env_path.mkdir()
            dummy_file.write_text(
                "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<urn:dummy> a owl:Ontology .\n",
                encoding="utf-8",
            )
            env1 = OntoEnv(path=env_path, recreate=True)
            env1.close()
            env = OntoEnv(path=env_path, read_only=True)
            self.assertTrue((env_path / ".ontoenv-snapshots").is_dir())
            with self.assertRaisesRegex(ValueError, "read-only"):
                env.add(dummy_file.resolve().as_uri())

    def test_init_no_config_requires_existing(self):
        original_cwd = Path.cwd()
        with tempfile.TemporaryDirectory() as td:
            os.chdir(td)
            try:
                with self.assertRaises(FileNotFoundError):
                    OntoEnv()
            finally:
                os.chdir(original_cwd)

    def test_connect_path_auto_initializes(self):
        with tempfile.TemporaryDirectory() as td:
            env_path = Path(td) / "no_env_here"
            env_path.mkdir()
            env = OntoEnv.connect(env_path)
            self.assertTrue((env_path / ".ontoenv-snapshots").is_dir())
            env.close()

    def test_init_temporary(self):
        with tempfile.TemporaryDirectory() as td:
            env_path = Path(td) / "temp_env_root"
            env = OntoEnv(temporary=True, root=str(env_path), strict=False)
            self.assertFalse((env_path / ".ontoenv-snapshots").exists())
            self.assertIsNone(env.store_path())
            try:
                env.add("http://example.com/nonexistent.ttl")
            except ValueError as e:
                self.assertNotIn("Cannot add to read-only store", str(e))
            except Exception:
                pass


if __name__ == "__main__":
    unittest.main()
