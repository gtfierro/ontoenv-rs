from __future__ import annotations

import sys

from ontoenv import run_cli as _run_cli


def main(argv: list[str] | None = None) -> int:
    code = _run_cli(argv if argv is not None else list(sys.argv))
    if code != 0:
        raise SystemExit(code)
    return 0
