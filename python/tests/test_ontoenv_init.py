import unittest
import shutil
import os
import tempfile
import re
from pathlib import Path
from ontoenv import OntoEnv


class TestOntoEnvInit(unittest.TestCase):
    def tearDown(self):
        # Clean up any accidental .ontoenv in cwd
        if Path(".ontoenv").exists():
            shutil.rmtree(".ontoenv")

    def test_init_recreate_new_dir(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            env_path = root / "new_env"
            self.assertFalse(env_path.exists())
            env = OntoEnv(path=env_path, recreate=True)
            self.assertTrue((env_path / ".ontoenv").is_dir())
            sp = env.store_path()
            self.assertIsNotNone(sp)
            self.assertTrue(Path(sp).is_dir())

    def test_init_recreate_existing_empty_dir(self):
        with tempfile.TemporaryDirectory() as td:
            env_path = Path(td) / "empty_env"
            env_path.mkdir()
            self.assertTrue(env_path.is_dir())
            env = OntoEnv(path=env_path, recreate=True)
            self.assertTrue((env_path / ".ontoenv").is_dir())
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
            self.assertEqual(env2.store_path(), str(env_path / ".ontoenv"))

    def test_init_recreate_existing_dir(self):
        with tempfile.TemporaryDirectory() as td:
            env_path = Path(td) / "existing_env"
            env_path.mkdir()
            env = OntoEnv(path=env_path, recreate=True)
            (env_path / ".ontoenv" / "dummy.txt").touch()
            self.assertTrue((env_path / ".ontoenv" / "dummy.txt").exists())
            # Recreate
            env = OntoEnv(path=env_path, recreate=True)
            self.assertTrue((env_path / ".ontoenv").is_dir())
            self.assertFalse((env_path / ".ontoenv" / "dummy.txt").exists())
            self.assertEqual(len(env.get_ontology_names()), 0)

    def test_init_read_only(self):
        with tempfile.TemporaryDirectory() as td:
            env_path = Path(td) / "existing_env"
            env_path.mkdir()
            env1 = OntoEnv(path=env_path, recreate=True)
            env1.close()
            env = OntoEnv(path=env_path, read_only=True)
            self.assertTrue((env_path / ".ontoenv").is_dir())
            with self.assertRaisesRegex(ValueError, "Cannot add to read-only store"):
                env.add("file:///dummy.ttl")

    def test_init_no_config_no_path_error(self):
        # Clean up potential leftover .ontoenv in cwd just in case
        if os.path.exists(".ontoenv"):
            if os.path.isfile(".ontoenv"):
                os.remove(".ontoenv")
            else:
                shutil.rmtree(".ontoenv")
        with self.assertRaisesRegex(ValueError, "OntoEnv directory not found at \"./.ontoenv\". You must provide a valid path or set recreate=True or temporary=True to create a new OntoEnv."):
            OntoEnv()  # No args

    def test_init_path_no_env_error(self):
        with tempfile.TemporaryDirectory() as td:
            env_path = Path(td) / "no_env_here"
            env_path.mkdir()
            self.assertFalse((env_path / ".ontoenv").exists())
            # Be tolerant of macOS /private prefix differences by matching only the tail.
            tail_pattern = rf'OntoEnv directory not found at: "(.*/)?{re.escape(env_path.name)}/\.ontoenv"'
            with self.assertRaisesRegex(ValueError, tail_pattern):
                OntoEnv(path=env_path)

    def test_init_temporary(self):
        with tempfile.TemporaryDirectory() as td:
            env_path = Path(td) / "temp_env_root"
            env = OntoEnv(temporary=True, root=str(env_path), strict=False)
            self.assertFalse((env_path / ".ontoenv").exists())
            self.assertIsNone(env.store_path())
            try:
                env.add("http://example.com/nonexistent.ttl")
            except ValueError as e:
                self.assertNotIn("Cannot add to read-only store", str(e))
            except Exception:
                pass


if __name__ == "__main__":
    unittest.main()
