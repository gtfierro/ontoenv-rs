#!/usr/bin/env python3
"""
Benchmark .ttl files vs rdf5d.

For each .ttl file in a directory, measures:
  - Size: original TTL vs generated .r5tu
  - Time: rdflib load/serialize vs r5tu build/stat (load)

Requires:
  - Python: rdflib (pip install rdflib)
  - r5tu CLI built with --features oxigraph (built automatically if not found)

Usage:
  python scripts/benchmark_ttl_dir.py DIR [-o results.csv] [--recursive]
                                         [--keep-artifacts]
                                         [--r5tu PATH] [--enable-mmap]

Notes:
  - The script tries to use target/release/r5tu if present; otherwise it will
    attempt to build it via `cargo build --release --features oxigraph[,mmap]`.
  - Load time for rdf5d is measured by timing `r5tu stat --file <file>`.
  - rdflib save time is for Graph.serialize() to Turtle.
"""

from __future__ import annotations

import argparse
import csv
import gc
import os
import shlex
import subprocess
import sys
import tempfile
import time
from pathlib import Path
import statistics as stats
from typing import Iterable, List, Optional, Tuple


def _r5tu_has_oxigraph_cli(r5tu_path: Path) -> bool:
    """Detect if the r5tu binary is built with the oxigraph CLI.

    The non-oxigraph build prints a message like:
      "r5tu CLI requires the 'oxigraph' feature. Try: ..."
    and does not expose clap subcommands.
    """
    try:
        proc = subprocess.run(
            [r5tu_path.as_posix(), "--help"], capture_output=True, text=True
        )
    except Exception:
        return False
    out = (proc.stdout or "") + (proc.stderr or "")
    if "requires the 'oxigraph' feature" in out:
        return False
    # Heuristic: help should mention subcommands like build-graph/stat
    return ("build-graph" in out) and ("stat" in out)


def find_or_build_r5tu(explicit_path: Optional[Path], enable_mmap: bool) -> Path:
    """Find existing r5tu binary or build it.

    Returns path to the binary, raising RuntimeError on failure.
    """
    if explicit_path:
        p = explicit_path.resolve()
        if not p.exists():
            raise RuntimeError(f"r5tu not found at {p}")
        if not _r5tu_has_oxigraph_cli(p):
            raise RuntimeError(
                f"r5tu at {p} does not expose the oxigraph-based CLI (build with --features oxigraph)"
            )
        if not _r5tu_supports_zstd(p):
            raise RuntimeError(
                f"r5tu at {p} does not support --zstd (build with --features zstd)."
            )
        return p

    # Prefer release build if present
    candidate = Path("target/release/r5tu")
    if candidate.exists() and _r5tu_has_oxigraph_cli(candidate) and _r5tu_supports_zstd(candidate):
        return candidate.resolve()

    # Try debug build
    candidate = Path("target/debug/r5tu")
    if candidate.exists() and _r5tu_has_oxigraph_cli(candidate) and _r5tu_supports_zstd(candidate):
        return candidate.resolve()

    # Build release with required features
    features = ["oxigraph", "zstd"]
    if enable_mmap:
        features.append("mmap")
    cmd = [
        "cargo",
        "build",
        "--release",
        "--features",
        ",".join(features),
    ]
    print("Building r5tu:", " ".join(shlex.quote(x) for x in cmd))
    try:
        subprocess.run(cmd, check=True)
    except subprocess.CalledProcessError as e:
        raise RuntimeError(
            "Failed to build r5tu. Ensure Rust toolchain is installed and network access is available for crates."
        ) from e

    candidate = Path("target/release/r5tu")
    if not candidate.exists() or not _r5tu_has_oxigraph_cli(candidate) or not _r5tu_supports_zstd(candidate):
        raise RuntimeError("r5tu binary not found after build")
    return candidate.resolve()


def _r5tu_supports_zstd(r5tu_path: Path) -> bool:
    """Try a tiny build-graph with --zstd to verify feature is enabled."""
    try:
        with tempfile.TemporaryDirectory(prefix="r5tu_zstd_check_") as tdir:
            tdirp = Path(tdir)
            ttl = tdirp / "t.ttl"
            out = tdirp / "t.r5tu"
            ttl.write_text("""
@prefix ex: <http://example.org/> .
ex:s ex:p ex:o .
""".strip())
            cmd = [
                r5tu_path.as_posix(),
                "build-graph",
                "--input",
                ttl.as_posix(),
                "--output",
                out.as_posix(),
                "--format",
                "turtle",
                "--zstd",
            ]
            proc = subprocess.run(cmd, capture_output=True, text=True)
            if proc.returncode != 0:
                combined = (proc.stdout or "") + (proc.stderr or "")
                return "zstd feature not enabled" not in combined
            return out.exists() and out.stat().st_size > 0
    except Exception:
        return False


def iter_ttl_files(root: Path, recursive: bool) -> Iterable[Path]:
    exts = {".ttl", ".turtle"}
    if recursive:
        for p in root.rglob("*"):
            if p.is_file() and p.suffix.lower() in exts:
                yield p
    else:
        for p in root.iterdir():
            if p.is_file() and p.suffix.lower() in exts:
                yield p


def measure_rdflib(ttl_path: Path, out_dir: Path, trials: int = 5) -> Tuple[List[float], List[float], int]:
    try:
        from rdflib import Graph
    except Exception as e:  # pragma: no cover
        raise RuntimeError(
            "rdflib is required. Install with: pip install rdflib"
        ) from e

    load_times: List[float] = []
    save_times: List[float] = []
    out_ttl_size = 0

    for i in range(trials):
        # Load
        g = Graph()
        t0 = time.perf_counter()
        g.parse(ttl_path.as_posix(), format="turtle")
        load_times.append(time.perf_counter() - t0)

        # Serialize
        gc.collect()
        out_ttl = out_dir / f"{ttl_path.stem}.rdflib.t{i}.out.ttl"
        t1 = time.perf_counter()
        g.serialize(destination=out_ttl.as_posix(), format="turtle")
        save_times.append(time.perf_counter() - t1)
        if out_ttl.exists():
            out_ttl_size = out_ttl.stat().st_size

        # Free graph for next iteration
        del g
        gc.collect()

    return load_times, save_times, out_ttl_size


def run_timed(cmd: List[str]) -> Tuple[float, int, str]:
    """Run command, return (seconds, exit_code, stderr+stdout)."""
    t0 = time.perf_counter()
    proc = subprocess.run(cmd, capture_output=True, text=True)
    dt = time.perf_counter() - t0
    out = (proc.stdout or "") + (proc.stderr or "")
    return dt, proc.returncode, out


def measure_r5tu(
    r5tu: Path, ttl_path: Path, out_dir: Path, trials: int = 5
) -> Tuple[List[float], List[float], int, Optional[str]]:
    # Build .r5tu
    build_times: List[float] = []
    stat_times: List[float] = []
    r5_size: int = 0
    last_stat_err: Optional[str] = None

    for i in range(trials):
        out_r5 = out_dir / f"{ttl_path.stem}.trial{i}.r5tu"
        if out_r5.exists():
            try:
                out_r5.unlink()
            except Exception:
                pass
        build_cmd = [
            r5tu.as_posix(),
            "build-graph",
            "--input",
            ttl_path.as_posix(),
            "--output",
            out_r5.as_posix(),
            "--format",
            "turtle",
            "--zstd",
        ]
        build_s, code, build_out = run_timed(build_cmd)
        if code != 0:
            raise RuntimeError(
                f"r5tu build-graph failed (code={code}) for {ttl_path}:\n{build_out}"
            )
        if not out_r5.exists() or out_r5.stat().st_size == 0:
            raise RuntimeError(
                f"r5tu did not produce a valid file: {out_r5} (check that r5tu supports oxigraph CLI)"
            )
        build_times.append(build_s)
        r5_size = out_r5.stat().st_size

        # Stat (load) time
        stat_cmd = [r5tu.as_posix(), "stat", "--file", out_r5.as_posix()]
        stat_s, code, stat_out = run_timed(stat_cmd)
        if code != 0:
            last_stat_err = stat_out
        stat_times.append(stat_s)

    return build_times, stat_times, r5_size, last_stat_err


def main() -> int:
    ap = argparse.ArgumentParser(description="Benchmark Turtle vs rdf5d")
    ap.add_argument("dir", type=Path, help="Directory containing .ttl/.turtle files")
    ap.add_argument(
        "-o",
        "--output",
        type=Path,
        default=Path("benchmark_results.csv"),
        help="CSV output file",
    )
    ap.add_argument(
        "--recursive",
        action="store_true",
        help="Recurse into subdirectories",
    )
    ap.add_argument(
        "--keep-artifacts",
        action="store_true",
        help="Keep generated .r5tu and rdflib .ttl outputs",
    )
    ap.add_argument(
        "--r5tu",
        type=Path,
        default=None,
        help="Path to r5tu binary (if not provided, attempts to find/build)",
    )
    ap.add_argument(
        "--enable-mmap",
        action="store_true",
        help="Build r5tu with mmap feature for mmap-based loading",
    )
    args = ap.parse_args()

    root = args.dir
    if not root.exists() or not root.is_dir():
        print(f"Error: '{root}' is not a directory", file=sys.stderr)
        return 2

    try:
        r5tu_bin = find_or_build_r5tu(args.r5tu, args.enable_mmap)
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        return 2

    ttl_files = list(iter_ttl_files(root, args.recursive))
    if not ttl_files:
        print("No .ttl/.turtle files found", file=sys.stderr)
        return 1

    # Work dir for outputs
    if args.keep_artifacts:
        out_root = Path("bench_artifacts")
        out_root.mkdir(parents=True, exist_ok=True)
        temp_mgr = None
    else:
        temp_mgr = tempfile.TemporaryDirectory(prefix="rdf5d_bench_")
        out_root = Path(temp_mgr.name)

    print(f"Using r5tu: {r5tu_bin}")
    print(f"Artifacts dir: {out_root}")
    results: List[dict] = []

    for i, ttl in enumerate(ttl_files, 1):
        try:
            print(f"[{i}/{len(ttl_files)}] {ttl}")
            ttl_size = ttl.stat().st_size
            file_out_dir = out_root / ttl.stem
            file_out_dir.mkdir(parents=True, exist_ok=True)

            # rdflib (repeat trials)
            rdflib_load_list, rdflib_save_list, rdflib_out_size = measure_rdflib(ttl, file_out_dir, trials=5)

            # rdf5d via r5tu (repeat trials)
            r5_build_list, r5_stat_list, r5_size, stat_err = measure_r5tu(r5tu_bin, ttl, file_out_dir, trials=5)

            # Compute means and stddevs
            rdflib_load_mean = stats.mean(rdflib_load_list)
            rdflib_load_std = stats.stdev(rdflib_load_list) if len(rdflib_load_list) > 1 else 0.0
            rdflib_save_mean = stats.mean(rdflib_save_list)
            rdflib_save_std = stats.stdev(rdflib_save_list) if len(rdflib_save_list) > 1 else 0.0
            r5_build_mean = stats.mean(r5_build_list)
            r5_build_std = stats.stdev(r5_build_list) if len(r5_build_list) > 1 else 0.0
            r5_stat_mean = stats.mean(r5_stat_list)
            r5_stat_std = stats.stdev(r5_stat_list) if len(r5_stat_list) > 1 else 0.0

            rdflib_total_list = [a + b for a, b in zip(rdflib_load_list, rdflib_save_list)]
            r5_total_list = [a + b for a, b in zip(r5_stat_list, r5_build_list)]
            rdflib_total_mean = stats.mean(rdflib_total_list)
            rdflib_total_std = stats.stdev(rdflib_total_list) if len(rdflib_total_list) > 1 else 0.0
            r5_total_mean = stats.mean(r5_total_list)
            r5_total_std = stats.stdev(r5_total_list) if len(r5_total_list) > 1 else 0.0

            save_speedup = (rdflib_save_mean / r5_build_mean) if r5_build_mean > 0 else 0.0
            load_speedup = (rdflib_load_mean / r5_stat_mean) if r5_stat_mean > 0 else 0.0
            total_speedup = (rdflib_total_mean / r5_total_mean) if r5_total_mean > 0 else 0.0
            row = {
                "file": ttl.as_posix(),
                "ttl_size": ttl_size,
                "rdflib_load_mean": rdflib_load_mean,
                "rdflib_load_std": rdflib_load_std,
                "rdflib_save_mean": rdflib_save_mean,
                "rdflib_save_std": rdflib_save_std,
                "rdflib_out_size": rdflib_out_size,
                "r5tu_build_mean": r5_build_mean,
                "r5tu_build_std": r5_build_std,
                "r5tu_load_mean": r5_stat_mean,
                "r5tu_load_std": r5_stat_std,
                "rdflib_total_mean": rdflib_total_mean,
                "rdflib_total_std": rdflib_total_std,
                "r5tu_total_mean": r5_total_mean,
                "r5tu_total_std": r5_total_std,
                "r5tu_size": r5_size,
                "size_ratio_r5tu_over_ttl": (r5_size / ttl_size) if ttl_size else 0.0,
                "size_ratio_r5tu_over_rdflib_out": (r5_size / rdflib_out_size) if rdflib_out_size else 0.0,
                "save_speedup_r5tu_over_rdflib": save_speedup,
                "load_speedup_r5tu_over_rdflib": load_speedup,
                "total_speedup_r5tu_over_rdflib": total_speedup,
            }
            if stat_err:
                row["r5tu_stat_error"] = stat_err.strip().splitlines()[-1][:200]
            results.append(row)
        except Exception as e:
            results.append({
                "file": ttl.as_posix(),
                "error": str(e),
            })

    # Write CSV
    fieldnames = [
        "file",
        "ttl_size",
        "r5tu_size",
        "rdflib_out_size",
        "rdflib_load_mean",
        "rdflib_load_std",
        "rdflib_save_mean",
        "rdflib_save_std",
        "r5tu_build_mean",
        "r5tu_build_std",
        "r5tu_load_mean",
        "r5tu_load_std",
        "rdflib_total_mean",
        "rdflib_total_std",
        "r5tu_total_mean",
        "r5tu_total_std",
        "size_ratio_r5tu_over_ttl",
        "size_ratio_r5tu_over_rdflib_out",
        "save_speedup_r5tu_over_rdflib",
        "load_speedup_r5tu_over_rdflib",
        "total_speedup_r5tu_over_rdflib",
        "r5tu_stat_error",
        "error",
    ]
    # Only include columns present
    present = [f for f in fieldnames if any(f in r for r in results)]
    with args.output.open("w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=present)
        w.writeheader()
        for r in results:
            # Format floats to 6 decimals for readability
            row_out = {}
            for k in present:
                v = r.get(k, "")
                if isinstance(v, float):
                    row_out[k] = f"{v:.6f}"
                else:
                    row_out[k] = v
            w.writerow(row_out)

    print(f"Wrote results to {args.output}")

    # Summary
    ok_rows = [r for r in results if "error" not in r]
    n_ok = len(ok_rows)
    n_err = len(results) - n_ok
    if n_ok:
        total_ttl = sum(r.get("ttl_size", 0) for r in ok_rows)
        total_r5 = sum(r.get("r5tu_size", 0) for r in ok_rows)
        total_rdflib_out = sum(r.get("rdflib_out_size", 0) for r in ok_rows)

        mean_ratio_vs_ttl = sum(r.get("size_ratio_r5tu_over_ttl", 0.0) for r in ok_rows) / n_ok
        mean_ratio_vs_rdflib = (
            sum(r.get("size_ratio_r5tu_over_rdflib_out", 0.0) for r in ok_rows) / n_ok
        ) if total_rdflib_out else 0.0

        mean_rdflib_total = sum(r.get("rdflib_total_mean", 0.0) for r in ok_rows) / n_ok
        mean_r5_total = sum(r.get("r5tu_total_mean", 0.0) for r in ok_rows) / n_ok
        mean_total_speedup = (
            sum(r.get("total_speedup_r5tu_over_rdflib", 0.0) for r in ok_rows) / n_ok
        )

        mean_rdflib_load = sum(r.get("rdflib_load_mean", 0.0) for r in ok_rows) / n_ok
        mean_r5_load = sum(r.get("r5tu_load_mean", 0.0) for r in ok_rows) / n_ok
        mean_load_speedup = (
            sum(r.get("load_speedup_r5tu_over_rdflib", 0.0) for r in ok_rows) / n_ok
        )

        mean_rdflib_save = sum(r.get("rdflib_save_mean", 0.0) for r in ok_rows) / n_ok
        mean_r5_save = sum(r.get("r5tu_build_mean", 0.0) for r in ok_rows) / n_ok
        mean_save_speedup = (
            sum(r.get("save_speedup_r5tu_over_rdflib", 0.0) for r in ok_rows) / n_ok
        )

        pct_saved_vs_ttl = (1 - (total_r5 / total_ttl)) * 100 if total_ttl else 0.0
        pct_saved_vs_rdflib = (
            (1 - (total_r5 / total_rdflib_out)) * 100 if total_rdflib_out else 0.0
        )

        print("\nSummary (rdf5d vs rdflib):")
        print(f"  Files processed: {n_ok} ok, {n_err} errors")
        print(f"  Size total (TTL → R5TU): {total_ttl} → {total_r5} bytes ({pct_saved_vs_ttl:.2f}% saved)")
        if total_rdflib_out:
            print(
                f"  Size total (rdflib TTL → R5TU): {total_rdflib_out} → {total_r5} bytes ({pct_saved_vs_rdflib:.2f}% saved)"
            )
        print(f"  Mean size ratio R5TU/TTL: {mean_ratio_vs_ttl:.3f}")
        if total_rdflib_out:
            print(f"  Mean size ratio R5TU/rdflib TTL: {mean_ratio_vs_rdflib:.3f}")
        print(
            f"  Load+Save total mean (rdflib vs r5tu): {mean_rdflib_total:.3f}s vs {mean_r5_total:.3f}s (speedup {mean_total_speedup:.2f}×)"
        )
        print(
            f"  Load mean (rdflib vs r5tu): {mean_rdflib_load:.3f}s vs {mean_r5_load:.3f}s (speedup {mean_load_speedup:.2f}×)"
        )
        print(
            f"  Save mean (rdflib vs r5tu): {mean_rdflib_save:.3f}s vs {mean_r5_save:.3f}s (speedup {mean_save_speedup:.2f}×)"
        )
    else:
        print("\nSummary: no successful runs to summarize")
    if not args.keep_artifacts and temp_mgr is not None:
        temp_mgr.cleanup()
    return 0


if __name__ == "__main__":
    sys.exit(main())
