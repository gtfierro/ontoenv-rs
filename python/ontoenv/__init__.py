"""Python package shim for the ontoenv extension.

This re-exports symbols from the compiled extension and adds
pure-Python helpers (e.g., the `init` module command).
"""

# Try both common names to tolerate different build configurations
try:  # prefer the extension named 'ontoenv'
    from .ontoenv import *  # type: ignore[attr-defined]
    from . import ontoenv as _ext  # type: ignore[attr-defined]
except Exception:  # fallback to '_ontoenv'
    from ._ontoenv import *  # type: ignore[attr-defined]
    from . import _ontoenv as _ext  # type: ignore[attr-defined]

__doc__ = getattr(_ext, "__doc__", None)
if hasattr(_ext, "__all__"):
    __all__ = _ext.__all__  # type: ignore[attr-defined]

