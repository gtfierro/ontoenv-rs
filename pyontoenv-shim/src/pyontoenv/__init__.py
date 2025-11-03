"""Compatibility wrapper that proxies to the real ``ontoenv`` package."""

from importlib import import_module
import sys as _sys

_ontoenv = import_module("ontoenv")

# Re-export the public surface.
from ontoenv import *  # type: ignore  # noqa: F401,F403

version = getattr(_ontoenv, "version", None)
__version__ = version if isinstance(version, str) else getattr(_ontoenv, "__version__", "0.0.0")

__all__ = getattr(_ontoenv, "__all__", [])

# Ensure version metadata is available via both imports.
if not hasattr(_ontoenv, "__version__"):
    setattr(_ontoenv, "__version__", __version__)

# Ensure importing `pyontoenv` yields the same module object as `ontoenv`.
_sys.modules[__name__] = _ontoenv
