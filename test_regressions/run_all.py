#!/usr/bin/env python3
# Depends on:
# - Cargo and the standalone release `gutch-cli` binary built from
#   `gutch_repo/gutch-cli/Cargo.toml`.
# - The tracked deterministic 128 MiB recipe and preparation script under
#   `gutch_repo/test_dataset/test_dataset_recipes` and `gutch_repo/test_dataset/ensure_test_dataset.py`.
# Uses paths:
# - Generated input and mutable state: `gutch_repo/test_dataset/.artifacts/mock-regression-128mib/`.
# - Durable output: `docs/generated/gutch_regression/`.
#
# Builds the latest release CLI, creates and reconstructs deterministic generated snapshots,
# archives every raw Gutch stats JSON file and Perfetto trace, and regenerates a directly openable run viewer.
# Steps:
# - Build release `gutch-cli` and select every generated target variant.
# - Capture and reconstruct each snapshot while retaining its raw stats JSON.
# - Publish one aggregate run document and regenerate the embedded-data viewer.

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT))

from scripts._common.paths import (
    get_build_path,
    get_default_cargo_target_dir,
)

SNAPSHOT_COUNT = 4
PRODUCT_NAME = "mock-regression-128mib"
RECIPE_PATH = REPO_ROOT / "gutch_repo" / "test_dataset" / "test_dataset_recipes" / "mock-regression-128mib.json"
GENERATED_ROOT = REPO_ROOT / "gutch_repo" / "test_dataset" / ".artifacts" / PRODUCT_NAME
REPORT_ROOT = REPO_ROOT / "docs" / "generated" / "gutch_regression"
RUNS_ROOT = REPORT_ROOT / "runs"
VIEWER_PATH = REPORT_ROOT / "index.html"
VIEWER_TEMPLATE_PATH = Path(__file__).with_name("_viewer.html")
WORK_ROOT = GENERATED_ROOT / "runs"
RUN_SCHEMA_VERSION = 1
RUN_DATA_PLACEHOLDER = "__GUTCH_REGRESSION_RUNS__"

def parse_snapshot_version(snapshot_path: Path) -> tuple[int, int, int]:
    name = snapshot_path.name
    raw_version = name.split("-", 1)[0]
    raw_parts = raw_version.split(".")
    if len(raw_parts) != 3 or any(not part.isdigit() for part in raw_parts):
        raise RuntimeError(f"Expected a numeric three-part generated snapshot version: {name}")
    return int(raw_parts[0]), int(raw_parts[1]), int(raw_parts[2])


def select_latest_snapshots(product_root: Path, snapshot_count: int = SNAPSHOT_COUNT) -> list[tuple[str, Path]]:
    snapshots = sorted(
        (snapshot_path for snapshot_path in product_root.iterdir() if snapshot_path.is_dir() and not snapshot_path.name.startswith(".")),
        key=parse_snapshot_version,
    )
    if len(snapshots) < snapshot_count:
        raise RuntimeError(f"Expected at least {snapshot_count} generated snapshots in {product_root}, found {len(snapshots)}")
    return [(".".join(str(part) for part in parse_snapshot_version(snapshot_path)), snapshot_path) for snapshot_path in snapshots[-snapshot_count:]]


def run_command(command: list[str], description: str) -> None:
    print(f"[{description}] {' '.join(command)}", flush=True)
    command_env = os.environ.copy()
    command_env["CARGO_TARGET_DIR"] = str(get_default_cargo_target_dir(REPO_ROOT))
    command_env["XDG_CONFIG_HOME"] = str(GENERATED_ROOT / "config")
    subprocess.run(command, cwd=REPO_ROOT, env=command_env, check=True)


def materialize_regression_product() -> Path:
    source_root = GENERATED_ROOT / "source"
    shutil.rmtree(source_root, ignore_errors=True)
    run_command(
        [
            sys.executable,
            str(REPO_ROOT / "gutch_repo" / "test_dataset" / "ensure_test_dataset.py"),
            "--fixtures-root", str(source_root),
            "--recipe", RECIPE_PATH.stem,
            "--force",
        ],
        "prepare deterministic 128 MiB fixture",
    )
    recipe = json.loads(RECIPE_PATH.read_text(encoding="utf-8"))
    if recipe.get("schema_version") != 1 or recipe.get("logical_size_bytes") != 128 * 1024 * 1024:
        raise RuntimeError(f"Invalid regression recipe: {RECIPE_PATH}")
    return source_root / str(recipe["name"])


def build_release_gutch_cli() -> Path:
    run_command(
        [
            "cargo",
            "build",
            "--release",
            "--manifest-path",
            str(REPO_ROOT / "gutch_repo" / "gutch-cli" / "Cargo.toml"),
            "--bin",
            "gutch-cli",
        ],
        "build release gutch-cli",
    )
    executable = get_build_path(REPO_ROOT, "gutch_cli_release")
    if not executable.is_file():
        raise RuntimeError(f"Release gutch-cli was not produced at {executable}")
    return executable


def read_trace_spans(trace_path: Path) -> list[tuple[str, float]]:
    document = json.loads(trace_path.read_text(encoding="utf-8"))
    events = document.get("traceEvents", [])
    if not isinstance(events, list):
        return []
    stacks: dict[tuple[object, object, str], list[int]] = {}
    spans: list[tuple[str, float]] = []
    for event in events:
        if not isinstance(event, dict):
            continue
        name = event.get("name")
        phase = event.get("ph")
        timestamp = event.get("ts")
        if not isinstance(name, str) or not isinstance(timestamp, int):
            continue
        if name == "cli.command":
            continue
        key = (event.get("pid"), event.get("tid"), name)
        if phase == "B":
            stacks.setdefault(key, []).append(timestamp)
        elif phase == "E" and stacks.get(key):
            spans.append((name, max(0, timestamp - stacks[key].pop()) / 1_000_000))
    return spans


def aggregate_trace_files(trace_paths: list[Path], root_name: str) -> dict[str, Any]:
    aggregates: dict[str, dict[str, float | int]] = {}
    for trace_path in trace_paths:
        for name, seconds in read_trace_spans(trace_path):
            aggregate = aggregates.setdefault(name, {"count": 0, "total_seconds": 0.0, "maximum_seconds": 0.0})
            aggregate["count"] = int(aggregate["count"]) + 1
            aggregate["total_seconds"] = float(aggregate["total_seconds"]) + seconds
            aggregate["maximum_seconds"] = max(float(aggregate["maximum_seconds"]), seconds)
    ordered_spans = [
        {"name": name, **aggregate}
        for name, aggregate in sorted(
            aggregates.items(),
            key=lambda item: float(item[1]["total_seconds"]),
            reverse=True,
        )
    ]
    root = aggregates.get(root_name, {})
    return {
        "trace_count": len(trace_paths),
        "total_seconds": float(root.get("total_seconds", 0.0)),
        "spans": ordered_spans,
    }


def required_snapshot_id(stats: dict[str, Any], stats_path: Path) -> str:
    snapshot_id = stats.get("snapshot_id")
    if not isinstance(snapshot_id, str) or not snapshot_id:
        raise RuntimeError(f"Gutch stats are missing snapshot_id: {stats_path}")
    return snapshot_id


def git_revision() -> str:
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=REPO_ROOT,
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
    except (OSError, subprocess.CalledProcessError):
        return "nogit"
    revision = result.stdout.strip()
    return revision or "nogit"


def new_run_identity() -> tuple[str, str]:
    run_started_at = datetime.now(timezone.utc)
    run_utc = run_started_at.isoformat(timespec="microseconds").replace("+00:00", "Z")
    run_id = f"{run_started_at.strftime('%Y%m%dT%H%M%S%fZ')}-{git_revision()}"
    return run_id, run_utc


def load_run_documents(runs_root: Path | None = None) -> list[dict[str, Any]]:
    if runs_root is None:
        runs_root = RUNS_ROOT
    if not runs_root.exists():
        return []

    documents: list[dict[str, Any]] = []
    for run_path in sorted(runs_root.glob("*/run.json")):
        try:
            document = json.loads(run_path.read_text(encoding="utf-8"))
        except OSError as error:
            print(f"warning: skipping inaccessible regression run {run_path}: {error}", file=sys.stderr)
            continue
        run_id = document.get("run_id")
        if not isinstance(run_id, str) or run_id != run_path.parent.name:
            raise RuntimeError(f"Regression run_id does not match its folder: {run_path}")
        documents.append(document)
    documents.sort(key=lambda document: str(document.get("run_utc", "")), reverse=True)
    return documents


def render_viewer_html(run_documents: list[dict[str, Any]], template_path: Path = VIEWER_TEMPLATE_PATH) -> str:
    template = template_path.read_text(encoding="utf-8")
    if template.count(RUN_DATA_PLACEHOLDER) != 1:
        raise RuntimeError(f"Viewer template must contain one {RUN_DATA_PLACEHOLDER} placeholder: {template_path}")
    embedded_runs = json.dumps(run_documents, ensure_ascii=True, separators=(",", ":")).replace("</", "<\\/")
    return template.replace(RUN_DATA_PLACEHOLDER, embedded_runs)


def write_viewer(run_documents: list[dict[str, Any]]) -> None:
    REPORT_ROOT.mkdir(parents=True, exist_ok=True)
    temporary_path = VIEWER_PATH.with_suffix(".tmp")
    temporary_path.write_text(render_viewer_html(run_documents), encoding="utf-8")
    temporary_path.replace(VIEWER_PATH)


def publish_completed_run(staged_archive: Path, run_document: dict[str, Any]) -> Path:
    run_id = str(run_document["run_id"])
    final_archive = RUNS_ROOT / run_id
    if final_archive.exists():
        raise RuntimeError(f"Regression run archive already exists: {final_archive}")

    staged_archive.mkdir(parents=True, exist_ok=True)
    (staged_archive / "run.json").write_text(
        json.dumps(run_document, indent=2, ensure_ascii=True) + "\n",
        encoding="utf-8",
    )
    existing_documents = load_run_documents()
    viewer_html = render_viewer_html([run_document, *existing_documents])

    RUNS_ROOT.mkdir(parents=True, exist_ok=True)
    shutil.move(str(staged_archive), final_archive)
    temporary_viewer_path = VIEWER_PATH.with_suffix(".tmp")
    temporary_viewer_path.write_text(viewer_html, encoding="utf-8")
    temporary_viewer_path.replace(VIEWER_PATH)
    return final_archive


def run_regression(executable: Path, selected_snapshots: list[tuple[str, Path]]) -> Path:
    WORK_ROOT.mkdir(parents=True, exist_ok=True)
    run_id, run_utc = new_run_identity()
    revision = run_id.rsplit("-", 1)[1]

    with tempfile.TemporaryDirectory(prefix="run-", dir=WORK_ROOT) as temporary_dir:
        run_root = Path(temporary_dir)
        repo_path = run_root / "repo"
        output_root = run_root / "extracted"
        archive_root = run_root / "archive"
        output_root.mkdir()
        archive_root.mkdir()
        run_command(
            [
                str(executable),
                "repo",
                "init",
                "--path",
                str(repo_path),
            ],
            "repo init",
        )

        snapshot_records: list[dict[str, Any]] = []
        for index, (version, snapshot_path) in enumerate(selected_snapshots, start=1):
            snapshot_archive = archive_root / "snapshots" / f"{index:02d}-{snapshot_path.name}"
            snapshot_archive.mkdir(parents=True)
            stats_path = snapshot_archive / "capture.json"
            trace_path = snapshot_archive / "capture.trace.json"
            run_command(
                [
                    str(executable),
                    "snapshot",
                    "capture",
                    "--source-dir",
                    str(snapshot_path),
                    "--repo",
                    str(repo_path),
                    "--yes",
                    "--tag",
                    f"version::{version}",
                    "--tag",
                    f"regression_variant::{snapshot_path.name}",
                    "--stats-json-out",
                    str(stats_path),
                    "--trace-out",
                    str(trace_path),
                ],
                f"capture {index}/{len(selected_snapshots)} {snapshot_path.name}",
            )
            stats = json.loads(stats_path.read_text(encoding="utf-8"))
            snapshot_records.append(
                {
                    "index": index,
                    "name": snapshot_path.name,
                    "version": version,
                    "snapshot_id": required_snapshot_id(stats, stats_path),
                    "capture_stats": stats_path.relative_to(archive_root).as_posix(),
                    "capture_trace": trace_path.relative_to(archive_root).as_posix(),
                }
            )

        selected_by_name = {path.name: path for _, path in selected_snapshots}
        for snapshot_record in snapshot_records:
            index = int(snapshot_record["index"])
            snapshot_name = str(snapshot_record["name"])
            snapshot_id = str(snapshot_record["snapshot_id"])
            snapshot_archive = archive_root / "snapshots" / f"{index:02d}-{snapshot_name}"
            stats_path = snapshot_archive / "reconstruct.json"
            trace_path = snapshot_archive / "reconstruct.trace.json"
            run_command(
                [
                    str(executable),
                    "snapshot",
                    "reconstruct",
                    "--repo",
                    str(repo_path),
                    "--snapshot-id",
                    snapshot_id,
                    "--destination",
                    str(output_root / snapshot_name),
                    "--stats-json-out",
                    str(stats_path),
                    "--trace-out",
                    str(trace_path),
                ],
                f"reconstruct {index}/{len(snapshot_records)} {snapshot_name}",
            )
            snapshot_record["reconstruct_stats"] = stats_path.relative_to(archive_root).as_posix()
            snapshot_record["reconstruct_trace"] = trace_path.relative_to(archive_root).as_posix()
            verify_stats_path = snapshot_archive / "verify.json"
            verify_trace_path = snapshot_archive / "verify.trace.json"
            run_command(
                [
                    str(executable), "reconstructed", "verify",
                    "--repo", str(repo_path),
                    "--snapshot-id", snapshot_id,
                    "--destination", str(output_root / snapshot_name),
                    "--stats-json-out", str(verify_stats_path),
                    "--trace-out", str(verify_trace_path),
                ],
                f"verify {index}/{len(snapshot_records)} {snapshot_name}",
            )
            compare_stats_path = snapshot_archive / "compare.json"
            run_command(
                [
                    str(executable), "path", "compare",
                    "--expected", str(selected_by_name[snapshot_name]),
                    "--actual", str(output_root / snapshot_name),
                    "--stats-json-out", str(compare_stats_path),
                ],
                f"compare {index}/{len(snapshot_records)} {snapshot_name}",
            )
            snapshot_record["verify_stats"] = verify_stats_path.relative_to(archive_root).as_posix()
            snapshot_record["verify_trace"] = verify_trace_path.relative_to(archive_root).as_posix()
            snapshot_record["compare_stats"] = compare_stats_path.relative_to(archive_root).as_posix()

        run_document = {
            "schema_version": RUN_SCHEMA_VERSION,
            "run_id": run_id,
            "run_utc": run_utc,
            "git_revision": revision,
            "product": {
                "name": PRODUCT_NAME,
                "snapshot_count": len(selected_snapshots),
                "first_snapshot": selected_snapshots[0][1].name,
                "last_snapshot": selected_snapshots[-1][1].name,
            },
            "operations": {
                "capture": {
                    "summary": aggregate_trace_files(
                        [archive_root / record["capture_trace"] for record in snapshot_records],
                        "capture.build",
                    ),
                    "traces": [record["capture_trace"] for record in snapshot_records],
                },
                "reconstruct": {
                    "summary": aggregate_trace_files(
                        [archive_root / record["reconstruct_trace"] for record in snapshot_records],
                        "snapshot.reconstruct.apply",
                    ),
                    "traces": [record["reconstruct_trace"] for record in snapshot_records],
                },
                "verify": {
                    "summary": aggregate_trace_files(
                        [archive_root / record["verify_trace"] for record in snapshot_records],
                        "reconstructed.verify",
                    ),
                    "traces": [record["verify_trace"] for record in snapshot_records],
                },
            },
            "snapshots": snapshot_records,
        }
        return publish_completed_run(archive_root, run_document)


def main() -> int:
    started_at = time.perf_counter()
    try:
        product_root = materialize_regression_product()
        selected_snapshots = select_latest_snapshots(product_root)
        print("Selected snapshots:", flush=True)
        for version, snapshot_path in selected_snapshots:
            print(f"  {version}: {snapshot_path}", flush=True)
        executable = build_release_gutch_cli()
        archive_path = run_regression(executable, selected_snapshots)
        print(f"Archived regression run at {archive_path}", flush=True)
        print(f"Updated regression viewer at {VIEWER_PATH}", flush=True)
        print(f"Completed in {time.perf_counter() - started_at:.2f} seconds", flush=True)
        return 0
    except (OSError, RuntimeError, subprocess.CalledProcessError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr, flush=True)
        return 1
    except KeyboardInterrupt:
        print("\nAborted.", file=sys.stderr, flush=True)
        return 1


if __name__ == "__main__":
    sys.exit(main())
