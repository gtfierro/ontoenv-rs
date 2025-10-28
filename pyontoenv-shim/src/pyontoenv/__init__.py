"""Compatibility wrapper that proxies to the real ``ontoenv`` package."""

from importlib import import_module

_ontoenv = import_module("ontoenv")

# Re-export the public surface.
from ontoenv import *  # type: ignore  # noqa: F401,F403

version = getattr(_ontoenv, "version", None)
__version__ = version if isinstance(version, str) else getattr(_ontoenv, "__version__", "0.0.0")

__all__ = getattr(_ontoenv, "__all__", [])
