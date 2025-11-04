#!/usr/bin/env python3
"""
Smoke-test the built pyontoenv shim against the ontoenv wheel.

Expected wheel locations (relative to repo root):
  - python/target/wheels/ontoenv-*.whl
  - pyontoenv-shim/dist/pyontoenv-*.whl
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def pick_wheel(path: Path, pattern: str) -> Path:
    matches = sorted(path.glob(pattern))
    if not matches:
        raise SystemExit(f"no wheels match '{pattern}' under {path}")
    return matches[-1]


def run(cmd: list[str], **kwargs) -> None:
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, check=True, **kwargs)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ontoenv-wheel", type=Path, default=None)
    parser.add_argument("--shim-wheel", type=Path, default=None)
    args = parser.parse_args()

    ontoenv_wheel = (
        args.ontoenv_wheel
        if args.ontoenv_wheel
        else pick_wheel(ROOT / "python" / "target" / "wheels", "ontoenv-*.whl")
    )
    shim_wheel = (
        args.shim_wheel
        if args.shim_wheel
        else pick_wheel(ROOT / "pyontoenv-shim" / "dist", "pyontoenv-*.whl")
    )

    with tempfile.TemporaryDirectory(prefix="pyontoenv-test-") as tmp:
        venv_dir = Path(tmp) / "venv"
        run([sys.executable, "-m", "venv", str(venv_dir)])

        if sys.platform == "win32":
            python = venv_dir / "Scripts" / "python.exe"
            pip = venv_dir / "Scripts" / "pip.exe"
        else:
            python = venv_dir / "bin" / "python"
            pip = venv_dir / "bin" / "pip"

        run([str(pip), "install", "--no-deps", str(ontoenv_wheel), str(shim_wheel)])
        run(
            [
                str(python),
                "-c",
                "import pyontoenv, ontoenv; "
                "from pyontoenv import OntoEnv; "
                "assert pyontoenv is ontoenv; "
                "assert callable(OntoEnv); "
                "print('pyontoenv version', pyontoenv.__version__)",
            ]
        )
        run([str(python), "-m", "ontoenv._cli", "--help"], stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
