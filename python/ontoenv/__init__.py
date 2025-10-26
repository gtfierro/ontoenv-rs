"""Python package shim for the ontoenv extension."""

# This is the name we set in python/Cargo.toml
from .pyontoenv import * # type: ignore[attr-defined]
from . import pyontoenv as _ext  # type: ignore[attr-defined]

__doc__ = getattr(_ext, "__doc__", None)
if hasattr(_ext, "__all__"):
    __all__ = _ext.__all__  # type: ignore[attr-defined]
