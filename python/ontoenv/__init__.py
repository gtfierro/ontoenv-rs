"""Python package shim for the ontoenv extension."""

# These symbols come from the Rust extension module built via maturin.
from ontoenv._native import OntoEnv, Ontology, run_cli, version  # type: ignore[attr-defined]
from ontoenv import _native as _ext  # type: ignore[attr-defined]

__doc__ = getattr(_ext, "__doc__", None)  # type: ignore[assignment]

# Export the main classes and functions
__all__ = ["OntoEnv", "Ontology", "run_cli", "version"]
