"""Hatchling 'code' version source: reads version from workspace Cargo.toml."""

from pathlib import Path
import re

_cargo = Path(__file__).resolve().parent.parent / "Cargo.toml"
_text = _cargo.read_text(encoding="utf-8")
_in_wp = False
__version__ = None
for _line in _text.splitlines():
    _s = _line.strip()
    if _s == "[workspace.package]":
        _in_wp = True
        continue
    if _in_wp and _s.startswith("["):
        break
    if _in_wp:
        _m = re.search(r'version\s*=\s*"([^"]+)"', _s)
        if _m:
            _v = _m.group(1)
            # Convert semver pre-release to PEP 440: 0.6.0-a2 → 0.6.0a2
            __version__ = re.sub(r"-([a-zA-Z])", r"\1", _v)
            break
if __version__ is None:
    raise RuntimeError(f"workspace.package.version not found in {_cargo}")
