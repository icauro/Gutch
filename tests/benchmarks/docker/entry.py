"""Container-side entry point for the fixed Linux Gutch benchmark."""

from __future__ import annotations

import argparse
from dataclasses import replace
import json
from pathlib import Path
import platform
import shutil
import sys
import zipfile


CONTAINER_DATASET_DIRECTORY = "gutch-container-blender-cache"


def _emit_progress(stage: str, detail: str) -> None:
    print(
        "CONTAINER_PROGRESS "
        + json.dumps({"stage": stage, "detail": detail}, separators=(",", ":")),
        flush=True,
    )


def _extract_package(package_path: Path, destination: Path) -> None:
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    with zipfile.ZipFile(package_path) as package:
        for member in package.infolist():
            member_path = (destination / member.filename).resolve()
            if destination.resolve() not in member_path.parents and member_path != destination.resolve():
                raise RuntimeError(f"benchmark package contains unsafe path: {member.filename}")
        package.extractall(destination)


def _new_run_root(artifacts_root: Path, before: set[Path]) -> Path:
    candidates = [
        path
        for path in artifacts_root.iterdir()
        if path.is_dir() and path not in before and (path / "results.json").is_file()
    ]
    if len(candidates) != 1:
        raise RuntimeError(f"could not identify exactly one container benchmark run under {artifacts_root}")
    return candidates[0]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--fixture", required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--transfer-concurrency", type=int)
    parser.add_argument("--transfer-max-concurrency", type=int)
    parser.add_argument("--keep-artifacts", action="store_true")
    args = parser.parse_args()

    if platform.system() != "Linux" or platform.machine().lower() not in {"x86_64", "amd64"}:
        raise RuntimeError("container Gutch benchmark requires Linux x86_64")

    workspace = args.workspace.resolve()
    workspace.mkdir(parents=True, exist_ok=True)
    code_root = workspace / "code"
    _extract_package(args.package.resolve(), code_root)
    sys.path.insert(0, str(code_root))

    from tests.datasets.prepare_blender import prepare_blender_product
    from tests.benchmarks.benchmark_config import load_benchmark_config
    from tests.benchmarks.scheduling.environment import load_benchmark_environment
    from comparison_benchmark_suite.support.commands import GutchCli
    from comparison_benchmark_suite.tools.config import load_fixture
    from comparison_benchmark_suite.tools.report import print_fixture_report
    from tests.benchmarks.support.s3 import S3Settings
    from tests.benchmarks.comparison.s3_cycle import (
        load_s3_cycle,
        run_s3_cycle,
    )
    from comparison_benchmark_suite.tools.workflow import run_fixture

    config = load_benchmark_config(args.config.resolve())
    load_benchmark_environment(args.config.resolve(), overwrite=True)
    _emit_progress("preparation", "benchmark configuration loaded")
    binary_path = args.binary.resolve()
    if not binary_path.is_file():
        raise FileNotFoundError(f"uploaded Linux gutch-cli was not found: {binary_path}")
    binary_path.chmod(binary_path.stat().st_mode | 0o111)


    dataset_root = Path("/tmp/gutch-benchmark/dataset") / CONTAINER_DATASET_DIRECTORY
    product = dataset_root / "Blender"
    recipe_path = code_root / "tests/datasets/corpus/blender/case.json"
    if config.blender_platforms != ("linux-x64",):
        raise RuntimeError("container Gutch benchmark requires dataset.blender_platforms: [linux-x64]")
    prepare_blender_product(
        recipe_path=recipe_path,
        product_dir=product,
        versions=config.blender_versions,
        platforms=config.blender_platforms,
        cache_dir=dataset_root / ".blender-archives",
        force=False,
    )
    _emit_progress("preparation", f"Blender product ready: {product}")

    fixture_path = (
        code_root

        / "tests/benchmarks"
        / "comparison"
        / "fixtures"
        / f"{args.fixture}.yaml"
    )
    fixture = load_fixture(fixture_path)
    options = None
    if fixture.runner == "gutch-s3":
        fixture, options = load_s3_cycle(fixture_path)
        if args.transfer_concurrency is not None or args.transfer_max_concurrency is not None:
            transfer_concurrency = (
                args.transfer_concurrency
                if args.transfer_concurrency is not None
                else options.transfer_concurrency
            )
            transfer_max_concurrency = (
                args.transfer_max_concurrency
                if args.transfer_max_concurrency is not None
                else options.transfer_max_concurrency
            )
            if transfer_concurrency < 1 or transfer_max_concurrency < 1:
                raise ValueError("container transfer concurrency values must be positive")
            if transfer_concurrency > transfer_max_concurrency:
                raise ValueError("container transfer concurrency exceeds max-concurrency")
            options = replace(
                options,
                transfer_concurrency=transfer_concurrency,
                transfer_max_concurrency=transfer_max_concurrency,
            )
    elif fixture.runner != "gutch-fixture":
        raise ValueError(f"fixture {fixture.name!r} is not a supported container Gutch fixture")
    # Perfetto is opened by the local orchestrator after the trace is downloaded.
    fixture = replace(fixture, open_perfetto=False)
    artifacts_root = workspace / "artifacts"
    before = {path for path in artifacts_root.iterdir()} if artifacts_root.is_dir() else set()
    settings = S3Settings.from_environment("GUTCH_S3")
    run_root: Path | None = None
    _emit_progress(
        "benchmark",
        f"starting {fixture.name} with {len(fixture.lanes)} configured lane(s)",
    )
    if fixture.runner == "gutch-s3":
        assert options is not None
        results = run_s3_cycle(
            fixture,
            product,
            GutchCli(binary_path),
            settings,
            artifacts_root,
            options=options,
            on_lane_complete=lambda result: _emit_progress(
                "lane", f"{result.label} completed"
            ),
            keep_artifacts=True,
            on_step=lambda detail: _emit_progress("step", detail),
        )
    else:
        results = run_fixture(
            fixture,
            product,
            GutchCli(binary_path),
            artifacts_root,
            on_lane_complete=lambda result: _emit_progress(
                "lane", f"{result.label} completed"
            ),
            keep_artifacts=True,
            on_step=lambda detail: _emit_progress("step", detail),
        )
    run_root = _new_run_root(artifacts_root, before)
    _emit_progress("complete", f"benchmark results ready: {run_root.name}")
    print_fixture_report(fixture, product, results)

    evidence_path = workspace / 'evidence.zip'
    with zipfile.ZipFile(evidence_path, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
        for path in run_root.rglob('*'):
            if path.is_file() and (args.keep_artifacts or 'payload' not in path.relative_to(run_root).parts):
                archive.write(path, path.relative_to(run_root).as_posix())
    manifest = {
        "evidence_path": str(evidence_path),
        "fixture": fixture.name,
        "product": str(product),
        "dataset_path": str(dataset_root),
        "run_root": str(run_root),
        "results_path": str(run_root / "results.json"),
        "trace_path": str(run_root / "trace.json"),
        "s3_prefix": f"{options.prefix_root}/{run_root.name}" if options else None,
        "results": [result.as_json() for result in results],
    }
    (workspace / "container-result.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, ValueError, zipfile.BadZipFile) as error:
        print(f"container benchmark error: {error}", file=sys.stderr)
        raise SystemExit(1)
