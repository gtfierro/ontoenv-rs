"""Sphinx configuration for the OntoEnv project."""

from __future__ import annotations

import os
import sys
from pathlib import Path

try:  # Python 3.11+
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - fallback for older interpreters
    import tomli as tomllib  # type: ignore


ROOT = Path(__file__).resolve().parent.parent
PROJECT_ROOT = ROOT
PYTHON_SRC = PROJECT_ROOT / "python"

# Ensure the Python bindings are importable when building docs locally.
sys.path.insert(0, str(PYTHON_SRC))


def _read_version() -> str:
    """Read the workspace version from Cargo.toml for a single source of truth."""
    cargo_toml = PROJECT_ROOT / "Cargo.toml"
    try:
        with cargo_toml.open("rb") as fh:
            cargo = tomllib.load(fh)
        return cargo.get("workspace", {}).get("package", {}).get("version", "0.0.0")
    except Exception:
        return "0.0.0"


project = "OntoEnv"
author = "OntoEnv developers"
release = _read_version()
version = release

extensions = [
    "sphinx.ext.autodoc",
    "sphinx.ext.autosummary",
    "sphinx.ext.napoleon",
    "sphinx.ext.intersphinx",
    "sphinx.ext.viewcode",
]

autosummary_generate = True
autodoc_typehints = "description"
autodoc_mock_imports = ["rdflib", "oxrdflib"]

templates_path = ["_templates"]
exclude_patterns: list[str] = [
    "_build",
    "_doctrees",
    "Thumbs.db",
    ".DS_Store",
    ".venv",
    ".venv/**",
]

html_theme = "furo"
html_static_path = ["_static"]
html_title = "OntoEnv Documentation"

intersphinx_mapping = {
    "python": ("https://docs.python.org/3", {}),
    "rdflib": ("https://rdflib.readthedocs.io/en/stable", {}),
}
