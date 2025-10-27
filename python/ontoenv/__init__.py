"""Python package shim for the ontoenv extension."""

# This is the name we set in python/Cargo.toml
from .pyontoenv import OntoEnv, Ontology, run_cli, version  # type: ignore[attr-defined]
from . import pyontoenv as _ext  # type: ignore[attr-defined]

__doc__ = getattr(_ext, "__doc__", None)  # type: ignore[assignment]

# Export the main classes and functions
__all__ = ["OntoEnv", "Ontology", "run_cli", "version"]
