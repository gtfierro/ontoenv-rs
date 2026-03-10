#!/usr/bin/env python3
"""Run the rdf5d Criterion suite across a feature and workload matrix.

The script orchestrates `cargo bench --bench rdf5d_bench`, varying:
  - crate feature sets such as plain, mmap, zstd, or mmap+zstd
  - single-graph triple counts
  - multi-graph counts and triples-per-graph
  - Criterion sampling knobs

It saves each run under a unique Criterion baseline and writes:
  - per-run logs
  - a machine-readable `summary.csv`
  - a machine-readable `summary.json`

Example:
  python scripts/run_criterion_matrix.py \
      --feature-set plain= \
      --feature-set mmap=mmap \
      --feature-set zstd=zstd \
      --feature-set mmap_zstd=mmap,zstd \
      --single-graph-triples 100,1000,10000,100000
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import re
import subprocess
import sys
from dataclasses import asdict, dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Iterable


DEFAULT_FEATURE_SETS = ["plain=", "mmap=mmap", "zstd=zstd", "mmap_zstd=mmap,zstd"]


@dataclass(frozen=True)
class FeatureSet:
    label: str
    features: tuple[str, ...]


def parse_int_list(raw: str, arg_name: str) -> list[int]:
    values: list[int] = []
    for part in raw.split(","):
        part = part.strip()
        if not part:
            continue
        try:
            value = int(part)
        except ValueError as exc:
            raise argparse.ArgumentTypeError(
                f"{arg_name} must be a comma-separated list of integers: {raw!r}"
            ) from exc
        if value <= 0:
            raise argparse.ArgumentTypeError(f"{arg_name} values must be positive: {raw!r}")
        values.append(value)
    if not values:
        raise argparse.ArgumentTypeError(f"{arg_name} must contain at least one integer")
    return values


def parse_feature_set(raw: str) -> FeatureSet:
    if "=" in raw:
        label, feature_csv = raw.split("=", 1)
    else:
        label, feature_csv = raw, raw
    label = slugify(label.strip() or "plain")
    features = tuple(feature.strip() for feature in feature_csv.split(",") if feature.strip())
    return FeatureSet(label=label, features=features)


def slugify(value: str) -> str:
    slug = re.sub(r"[^A-Za-z0-9]+", "_", value.strip()).strip("_").lower()
    return slug or "plain"


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--bench",
        default="rdf5d_bench",
        help="Criterion bench target to execute",
    )
    parser.add_argument(
        "--feature-set",
        action="append",
        default=[],
        help=(
            "Feature set in LABEL=feat1,feat2 form. Repeat to add multiple runs. "
            "Use LABEL= for a no-feature run."
        ),
    )
    parser.add_argument(
        "--single-graph-triples",
        type=lambda raw: parse_int_list(raw, "--single-graph-triples"),
        default=parse_int_list("100,1000,10000", "--single-graph-triples"),
        help="Comma-separated triple counts for single-graph benchmarks",
    )
    parser.add_argument(
        "--graph-counts",
        type=lambda raw: parse_int_list(raw, "--graph-counts"),
        default=parse_int_list("5,20,100", "--graph-counts"),
        help="Comma-separated graph counts for metadata lookup benchmarks",
    )
    parser.add_argument(
        "--graph-triples-per-graph",
        type=lambda raw: parse_int_list(raw, "--graph-triples-per-graph"),
        default=parse_int_list("50", "--graph-triples-per-graph"),
        help="Comma-separated triples-per-graph values for lookup benchmarks",
    )
    parser.add_argument(
        "--enumerate-all-graph-counts",
        type=lambda raw: parse_int_list(raw, "--enumerate-all-graph-counts"),
        default=parse_int_list("5,20,100,500", "--enumerate-all-graph-counts"),
        help="Comma-separated graph counts for enumerate_all benchmarks",
    )
    parser.add_argument(
        "--enumerate-all-triples-per-graph",
        type=lambda raw: parse_int_list(raw, "--enumerate-all-triples-per-graph"),
        default=parse_int_list("10", "--enumerate-all-triples-per-graph"),
        help="Comma-separated triples-per-graph values for enumerate_all benchmarks",
    )
    parser.add_argument(
        "--roundtrip-graph-counts",
        type=lambda raw: parse_int_list(raw, "--roundtrip-graph-counts"),
        default=parse_int_list("3", "--roundtrip-graph-counts"),
        help="Comma-separated graph counts for roundtrip benchmarks",
    )
    parser.add_argument(
        "--roundtrip-triples-per-graph",
        type=lambda raw: parse_int_list(raw, "--roundtrip-triples-per-graph"),
        default=parse_int_list("1000,10000", "--roundtrip-triples-per-graph"),
        help="Comma-separated triples-per-graph values for roundtrip benchmarks",
    )
    parser.add_argument(
        "--sample-size",
        type=int,
        default=100,
        help="Criterion sample size",
    )
    parser.add_argument(
        "--measurement-time",
        type=float,
        default=5.0,
        help="Criterion measurement time in seconds",
    )
    parser.add_argument(
        "--warm-up-time",
        type=float,
        default=3.0,
        help="Criterion warm-up time in seconds",
    )
    parser.add_argument(
        "--filter",
        default="",
        help="Optional Criterion benchmark name filter passed to cargo bench",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        help="Directory for logs and summaries. Defaults to benchmark_runs/<timestamp>",
    )
    parser.add_argument(
        "--target-dir",
        type=Path,
        help="Cargo target dir to use for benchmark builds. Defaults to ./target",
    )
    parser.add_argument(
        "--keep-going",
        action="store_true",
        help="Continue running remaining feature sets after a failed bench run",
    )
    parser.add_argument(
        "--noplot",
        action="store_true",
        help="Pass --noplot to Criterion to reduce report generation time",
    )
    return parser.parse_args()


def join_csv(values: Iterable[int]) -> str:
    return ",".join(str(value) for value in values)


def baseline_name(index: int, feature_set: FeatureSet) -> str:
    return f"matrix_{index:02d}_{feature_set.label}"


def run_manifest(
    args: argparse.Namespace,
    feature_set: FeatureSet,
    baseline: str,
    command: list[str],
    env_overrides: dict[str, str],
    log_path: Path,
    exit_code: int | None = None,
) -> dict[str, Any]:
    return {
        "label": feature_set.label,
        "features": list(feature_set.features),
        "baseline": baseline,
        "command": command,
        "filter": args.filter,
        "sample_size": args.sample_size,
        "measurement_time_s": args.measurement_time,
        "warm_up_time_s": args.warm_up_time,
        "env": env_overrides,
        "log_path": str(log_path),
        "exit_code": exit_code,
    }


def criterion_env(args: argparse.Namespace) -> dict[str, str]:
    return {
        "RDF5D_BENCH_SINGLE_GRAPH_TRIPLES": join_csv(args.single_graph_triples),
        "RDF5D_BENCH_GRAPH_COUNTS": join_csv(args.graph_counts),
        "RDF5D_BENCH_GRAPH_TRIPLES_PER_GRAPH": join_csv(args.graph_triples_per_graph),
        "RDF5D_BENCH_ENUMERATE_ALL_GRAPH_COUNTS": join_csv(args.enumerate_all_graph_counts),
        "RDF5D_BENCH_ENUMERATE_ALL_TRIPLES_PER_GRAPH": join_csv(
            args.enumerate_all_triples_per_graph
        ),
        "RDF5D_BENCH_ROUNDTRIP_GRAPH_COUNTS": join_csv(args.roundtrip_graph_counts),
        "RDF5D_BENCH_ROUNDTRIP_TRIPLES_PER_GRAPH": join_csv(args.roundtrip_triples_per_graph),
    }


def build_command(args: argparse.Namespace, feature_set: FeatureSet, baseline: str) -> list[str]:
    command = ["cargo", "bench", "--bench", args.bench]
    if feature_set.features:
        command.extend(["--features", ",".join(feature_set.features)])
    if args.filter:
        command.append(args.filter)
    command.extend(
        [
            "--",
            "--save-baseline",
            baseline,
            "--sample-size",
            str(args.sample_size),
            "--measurement-time",
            str(args.measurement_time),
            "--warm-up-time",
            str(args.warm_up_time),
        ]
    )
    if args.noplot:
        command.append("--noplot")
    return command


def collect_baseline_rows(
    criterion_root: Path,
    baseline: str,
    feature_set: FeatureSet,
    args: argparse.Namespace,
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for benchmark_json in sorted(criterion_root.rglob("benchmark.json")):
        if benchmark_json.parent.name != baseline:
            continue
        estimates_path = benchmark_json.with_name("estimates.json")
        if not estimates_path.exists():
            continue
        benchmark = json.loads(benchmark_json.read_text())
        estimates = json.loads(estimates_path.read_text())
        throughput_kind, throughput_value = extract_throughput(benchmark.get("throughput"))
        rows.append(
            {
                "feature_set": feature_set.label,
                "features": ",".join(feature_set.features),
                "baseline": baseline,
                "filter": args.filter,
                "group_id": benchmark.get("group_id"),
                "function_id": benchmark.get("function_id"),
                "value_str": benchmark.get("value_str"),
                "full_id": benchmark.get("full_id"),
                "directory_name": benchmark.get("directory_name"),
                "throughput_kind": throughput_kind,
                "throughput_value": throughput_value,
                "mean_ns": estimate_point(estimates, "mean"),
                "mean_lower_ns": estimate_ci(estimates, "mean", "lower_bound"),
                "mean_upper_ns": estimate_ci(estimates, "mean", "upper_bound"),
                "median_ns": estimate_point(estimates, "median"),
                "median_lower_ns": estimate_ci(estimates, "median", "lower_bound"),
                "median_upper_ns": estimate_ci(estimates, "median", "upper_bound"),
                "slope_ns": estimate_point(estimates, "slope"),
                "slope_lower_ns": estimate_ci(estimates, "slope", "lower_bound"),
                "slope_upper_ns": estimate_ci(estimates, "slope", "upper_bound"),
                "std_dev_ns": estimate_point(estimates, "std_dev"),
                "single_graph_triples": join_csv(args.single_graph_triples),
                "graph_counts": join_csv(args.graph_counts),
                "graph_triples_per_graph": join_csv(args.graph_triples_per_graph),
                "enumerate_all_graph_counts": join_csv(args.enumerate_all_graph_counts),
                "enumerate_all_triples_per_graph": join_csv(
                    args.enumerate_all_triples_per_graph
                ),
                "roundtrip_graph_counts": join_csv(args.roundtrip_graph_counts),
                "roundtrip_triples_per_graph": join_csv(args.roundtrip_triples_per_graph),
            }
        )
    return rows


def extract_throughput(raw: Any) -> tuple[str, Any]:
    if isinstance(raw, dict) and len(raw) == 1:
        [(kind, value)] = raw.items()
        return kind, value
    return "", ""


def estimate_point(estimates: dict[str, Any], name: str) -> Any:
    value = estimates.get(name)
    if not isinstance(value, dict):
        return ""
    return value.get("point_estimate", "")


def estimate_ci(estimates: dict[str, Any], name: str, bound: str) -> Any:
    value = estimates.get(name)
    if not isinstance(value, dict):
        return ""
    ci = value.get("confidence_interval")
    if not isinstance(ci, dict):
        return ""
    return ci.get(bound, "")


def write_json(path: Path, payload: Any) -> None:
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def write_csv(path: Path, rows: list[dict[str, Any]]) -> None:
    fieldnames: list[str] = []
    for row in rows:
        for key in row.keys():
            if key not in fieldnames:
                fieldnames.append(key)
    with path.open("w", newline="") as csv_file:
        writer = csv.DictWriter(csv_file, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


def main() -> int:
    args = parse_args()
    root = repo_root()
    target_dir = (args.target_dir or (root / "target")).resolve()
    target_dir.mkdir(parents=True, exist_ok=True)
    criterion_root = target_dir / "criterion"

    feature_sets = [parse_feature_set(raw) for raw in (args.feature_set or DEFAULT_FEATURE_SETS)]
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    output_dir = args.output_dir or (root / "benchmark_runs" / f"criterion_matrix_{timestamp}")
    output_dir.mkdir(parents=True, exist_ok=True)
    logs_dir = output_dir / "logs"
    manifests_dir = output_dir / "manifests"
    logs_dir.mkdir(parents=True, exist_ok=True)
    manifests_dir.mkdir(parents=True, exist_ok=True)

    env_overrides = criterion_env(args)
    all_rows: list[dict[str, Any]] = []
    run_results: list[dict[str, Any]] = []
    failures: list[dict[str, Any]] = []

    for index, feature_set in enumerate(feature_sets, start=1):
        baseline = baseline_name(index, feature_set)
        command = build_command(args, feature_set, baseline)
        log_path = logs_dir / f"{baseline}.log"
        manifest_path = manifests_dir / f"{baseline}.json"

        run_env = os.environ.copy()
        run_env.update(env_overrides)
        run_env["CARGO_TARGET_DIR"] = str(target_dir)

        proc = subprocess.run(
            command,
            cwd=root,
            env=run_env,
            capture_output=True,
            text=True,
        )
        log_path.write_text((proc.stdout or "") + (proc.stderr or ""))

        manifest = run_manifest(
            args=args,
            feature_set=feature_set,
            baseline=baseline,
            command=command,
            env_overrides=env_overrides,
            log_path=log_path,
            exit_code=proc.returncode,
        )
        write_json(manifest_path, manifest)
        run_results.append(manifest)

        if proc.returncode != 0:
            failures.append(manifest)
            if not args.keep_going:
                break
            continue

        rows = collect_baseline_rows(criterion_root, baseline, feature_set, args)
        all_rows.extend(rows)

    write_json(output_dir / "runs.json", run_results)
    write_json(output_dir / "summary.json", all_rows)
    write_csv(output_dir / "summary.csv", all_rows)

    final_manifest = {
        "generated_at": datetime.now().isoformat(),
        "repo_root": str(root),
        "target_directory": str(target_dir),
        "criterion_root": str(criterion_root),
        "feature_sets": [asdict(feature_set) for feature_set in feature_sets],
        "output_dir": str(output_dir),
        "failures": failures,
        "rows": len(all_rows),
    }
    write_json(output_dir / "manifest.json", final_manifest)

    print(f"wrote summary to {output_dir / 'summary.csv'}")
    print(f"wrote raw run metadata to {output_dir / 'runs.json'}")
    if failures:
        print(
            f"{len(failures)} run(s) failed; inspect {output_dir / 'manifest.json'} and logs/",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
