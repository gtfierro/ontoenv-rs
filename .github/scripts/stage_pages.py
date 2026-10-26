#!/usr/bin/env python3
"""Keep published Sphinx versions, a development build, and a stable root alias."""

from __future__ import annotations

import argparse
import html
import json
import posixpath
import re
import shutil
from pathlib import Path


TAG = re.compile(r"v?\d+\.\d+\.\d+(?:[-.][A-Za-z0-9][A-Za-z0-9.-]*)?")
STABLE = re.compile(r"v?(\d+)\.(\d+)\.(\d+)")
MARKER = re.compile(r"\n?<!-- ontoenv-versions:start -->.*?<!-- ontoenv-versions:end -->", re.S)


def copy_docs(source: Path, destination: Path) -> None:
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(source, destination, ignore=shutil.ignore_patterns(".git"))


def stage(html_dir: Path, history: Path, output: Path, tag: str = "") -> dict:
    if tag and not TAG.fullmatch(tag):
        raise ValueError(f"Invalid documentation version tag: {tag!r}")
    if not (html_dir / "index.html").is_file():
        raise ValueError(f"No built documentation in {html_dir}")
    # These paths are workflow-controlled; reject overlap before deleting anything.
    paths = [p.resolve() for p in (html_dir, history, output)]
    if any(a == b or a in b.parents or b in a.parents
           for i, a in enumerate(paths) for b in paths[i + 1:]):
        raise ValueError("Build, history and output directories must not overlap")
    if history.exists():
        copy_docs(history, output)
    else:
        output.mkdir(parents=True, exist_ok=True)

    destination = output / "versions" / tag if tag else output / "dev"
    copy_docs(html_dir, destination)
    versions_dir = output / "versions"
    versions_dir.mkdir(exist_ok=True)
    tags = sorted(p.name for p in versions_dir.iterdir()
                  if p.is_dir() and TAG.fullmatch(p.name) and (p / "index.html").is_file())
    stable = sorted((t for t in tags if STABLE.fullmatch(t)),
                    key=lambda t: tuple(map(int, STABLE.fullmatch(t).groups())), reverse=True)
    entries = []
    for version_tag in reversed(tags):
        kind = "stable" if STABLE.fullmatch(version_tag) else "prerelease"
        label = version_tag.removeprefix("v")
        if kind == "prerelease":
            label += " (prerelease)"
        elif version_tag == stable[0]:
            label += " (stable)"
        entries.append({"label": label, "path": f"versions/{version_tag}/", "kind": kind})
    if (output / "dev" / "index.html").is_file():
        entries.append({"label": "Development (main)", "path": "dev/", "kind": "development"})

    default_path = f"versions/{stable[0]}/" if stable else (
        "dev/" if (output / "dev").exists() else f"versions/{tag}/")
    # Refresh only the root alias. Published versions and development stay intact.
    for child in output.iterdir():
        if child.name in {"versions", "dev", "CNAME"}:
            continue
        if child.is_dir():
            shutil.rmtree(child)
        else:
            child.unlink()
    for child in (output / default_path).iterdir():
        if child.is_dir():
            shutil.copytree(child, output / child.name)
        else:
            shutil.copy2(child, output / child.name)

    manifest = {"default": default_path, "versions": entries}
    (output / "versions.json").write_text(json.dumps(manifest, indent=2) + "\n")
    links = "\n".join(f'<li><a href="../{html.escape(e["path"])}">{html.escape(e["label"])}</a></li>'
                      for e in entries)
    (versions_dir / "index.html").write_text(
        '<!doctype html><html lang="en"><meta charset="utf-8">'
        '<meta name="viewport" content="width=device-width,initial-scale=1">'
        '<title>OntoEnv documentation versions</title><body>'
        '<h1>OntoEnv documentation versions</h1><ul>' + links + '</ul></body></html>\n')
    static = output / "_static"
    static.mkdir(exist_ok=True)
    asset_dir = Path(__file__).resolve().parents[2] / "docs" / "_static"
    for name in ("version-switcher.js", "version-switcher.css"):
        shutil.copy2(asset_dir / name, static / name)
    for page in output.rglob("*.html"):
        contents = MARKER.sub("", page.read_text())
        relative = page.parent.relative_to(output).as_posix()
        js = posixpath.relpath("_static/version-switcher.js", relative)
        css = posixpath.relpath("_static/version-switcher.css", relative)
        widget = ('\n<!-- ontoenv-versions:start -->'
                  f'<link rel="stylesheet" href="{css}">'
                  f'<script defer src="{js}"></script>'
                  '<!-- ontoenv-versions:end -->\n')
        page.write_text(contents.replace("</body>", widget + "</body>"))
    (output / ".nojekyll").touch()
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--html", required=True, type=Path)
    parser.add_argument("--history", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--tag", default="")
    args = parser.parse_args()
    print(json.dumps(stage(args.html, args.history, args.output, args.tag), indent=2))
