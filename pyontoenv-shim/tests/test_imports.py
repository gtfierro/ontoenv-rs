import sys
import unittest


class PyOntoEnvImportTests(unittest.TestCase):
    def test_pyontoenv_aliases_ontoenv(self) -> None:
        import pyontoenv  # noqa: F401
        import ontoenv

        self.assertIs(sys.modules["pyontoenv"], ontoenv)
        self.assertTrue(hasattr(ontoenv, "OntoEnv"))
        self.assertIsInstance(getattr(ontoenv, "__version__", None), str)


if __name__ == "__main__":
    unittest.main()
