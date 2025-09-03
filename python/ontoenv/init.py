"""Simple Python-only frontend to initialize an OntoEnv.

Usage examples:

  python -m ontoenv.init
  python -m ontoenv.init --temporary --root .
  python -m ontoenv.init --path ./proj --read-only

The flags mirror the OntoEnv(...) constructor.
"""

from __future__ import annotations

import argparse
from pathlib import Path
from typing import List, Optional

from . import OntoEnv


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Initialize an OntoEnv environment")
    p.add_argument("--path", type=str, default=None, help="Root directory for the env")
    p.add_argument("--read-only", dest="read_only", action="store_true", help="Open env in read-only mode")

    p.add_argument(
        "--search-dir",
        dest="search_directories",
        action="append",
        help="Directory to search for ontologies (repeatable)",
    )
    p.add_argument("--require-ontology-names", action="store_true", help="Require ontology names")
    p.add_argument("--strict", action="store_true", help="Enable strict mode")
    p.add_argument("--offline", action="store_true", help="Disable network access for resolution")
    p.add_argument("--resolution-policy", default="default", help="Resolution policy name")
    p.add_argument("--root", default=".", help="Root directory for discovery (when not recreating)")
    p.add_argument("--include", dest="includes", action="append", help="Include pattern (repeatable)")
    p.add_argument("--exclude", dest="excludes", action="append", help="Exclude pattern (repeatable)")
    p.add_argument("--temporary", action="store_true", help="Use an in-memory environment")
    p.add_argument("--no-search", dest="no_search", action="store_true", help="Disable local search")
    return p


def main(argv: Optional[List[str]] = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    kwargs = dict(
        read_only=args.read_only,
        search_directories=args.search_directories,
        require_ontology_names=args.require_ontology_names,
        strict=args.strict,
        offline=args.offline,
        resolution_policy=args.resolution_policy,
        root=args.root,
        includes=args.includes,
        excludes=args.excludes,
        temporary=args.temporary,
        no_search=args.no_search,
    )

    if args.path is not None:
        kwargs["path"] = Path(args.path)
    kwargs["recreate"] = True

    env = None
    try:
        env = OntoEnv(**kwargs)
        # Persist to disk when applicable
        if not args.temporary:
            env.flush()
        store = env.store_path()
        if store:
            print(store)
        return 0
    except Exception as e:  # surface a clean message and non-zero exit
        parser.error(str(e))
        return 2
    finally:
        if env is not None:
            try:
                env.close()
            except Exception:
                pass


if __name__ == "__main__":
    raise SystemExit(main())

