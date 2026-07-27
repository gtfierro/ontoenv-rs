"""Python package shim for the ontoenv extension."""

# These symbols come from the Rust extension module built via maturin.
from ontoenv._native import (  # type: ignore[attr-defined]
    CatalogRecoveryError,
    UnresolvedImportError,
    ExternalStoreChangedError,
    OntoEnv,
    Ontology,
    StoreCapabilityError,
    SyncReport,
    is_debug_build,
    run_cli,
    version,
)
from ontoenv import _native as _ext  # type: ignore[attr-defined]
from ontoenv.rdflib_store import (
    OntoEnvStore,
    ViewGraph,
)

__doc__ = getattr(_ext, "__doc__", None)  # type: ignore[assignment]

# Export the main classes and functions
__all__ = [
    "OntoEnv",
    "Ontology",
    "SyncReport",
    "ExternalStoreChangedError",
    "CatalogRecoveryError",
    "UnresolvedImportError",
    "StoreCapabilityError",
    "OntoEnvStore",
    "ViewGraph",
    "is_debug_build",
    "run_cli",
    "version",
]
