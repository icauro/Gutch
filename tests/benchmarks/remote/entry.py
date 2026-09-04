"""Remote-side entry point for the fixed Linux Gutch benchmark."""

from __future__ import annotations

import argparse
from dataclasses import replace
import json
from pathlib import Path
import platform
import shutil
import sys
import zipfile


REMOTE_DATASET_DIRECTORY = "gutch-remote-blender-cache"


def _emit_progress(stage: str, detail: str) -> None:
    print(
        "REMOTE_PROGRESS "
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
        raise RuntimeError(f"could not identify exactly one remote benchmark run under {artifacts_root}")
    return candidates[0]


def _cleanup_s3(settings, prefix: str) -> None:
    from gutch_repo.tests.benchmarks.s3_throughput.config import StorageTarget
    from gutch_repo.tests.benchmarks.s3_throughput.workflow import (
        run_abort_multipart_uploads,
        run_delete,
    )

    target = StorageTarget(
        "remote-main-cleanup",
        "S3_MAIN",
        settings.endpoint,
        settings.region,
        settings.bucket,
        True,
    )
    run_abort_multipart_uploads(target, prefix=prefix)
    run_delete(target, prefix=prefix)


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
        raise RuntimeError("remote Gutch benchmark requires Linux x86_64")

    workspace = args.workspace.resolve()
    workspace.mkdir(parents=True, exist_ok=True)
    code_root = workspace / "code"
    _extract_package(args.package.resolve(), code_root)
    sys.path.insert(0, str(code_root))

    from prepare_test_dataset import prepare_blender_product
    from gutch_repo.tests.benchmarks.benchmark_config import load_benchmark_config
    from gutch_repo.tests.benchmarks.common.environment import load_benchmark_environment
    from gutch_repo.tests.benchmarks.gutch_fixture_runner.commands import GutchCli
    from gutch_repo.tests.benchmarks.gutch_fixture_runner.config import load_fixture
    from gutch_repo.tests.benchmarks.gutch_fixture_runner.report import print_fixture_report
    from gutch_repo.tests.benchmarks.gutch_fixture_runner.s3 import S3Settings
    from gutch_repo.tests.benchmarks.gutch_fixture_runner.s3_cycle import (
        load_s3_cycle,
        run_s3_cycle,
    )
    from gutch_repo.tests.benchmarks.gutch_fixture_runner.workflow import run_fixture

    config = load_benchmark_config(args.config.resolve())
    load_benchmark_environment(args.config.resolve(), overwrite=True)
    _emit_progress("preparation", "benchmark configuration loaded")
    binary_path = args.binary.resolve()
    if not binary_path.is_file():
        raise FileNotFoundError(f"uploaded Linux hostapp-cli was not found: {binary_path}")
    binary_path.chmod(binary_path.stat().st_mode | 0o111)

    dataset_root = Path(config.remote_fixtures_root) / REMOTE_DATASET_DIRECTORY
    product = dataset_root / "Blender"
    recipe_path = code_root / "gutch_repo" / "tests" / "test_dataset_recipes" / "blender-official.json"
    if config.blender_platforms != ("linux-x64",):
        raise RuntimeError("remote Gutch benchmark requires dataset.blender_platforms: [linux-x64]")
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
        / "gutch_repo"
        / "tests"
        / "benchmarks"
        / "gutch_fixture_runner"
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
                raise ValueError("remote transfer concurrency values must be positive")
            if transfer_concurrency > transfer_max_concurrency:
                raise ValueError("remote transfer concurrency exceeds max-concurrency")
            options = replace(
                options,
                transfer_concurrency=transfer_concurrency,
                transfer_max_concurrency=transfer_max_concurrency,
            )
    elif fixture.runner != "gutch-fixture":
        raise ValueError(f"fixture {fixture.name!r} is not a supported remote Gutch fixture")
    # Perfetto is opened by the local orchestrator after the trace is downloaded.
    fixture = replace(fixture, open_perfetto=False)
    artifacts_root = workspace / "artifacts"
    before = {path for path in artifacts_root.iterdir()} if artifacts_root.is_dir() else set()
    settings = S3Settings.from_environment("S3_MAIN")
    run_root: Path | None = None
    try:
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

        manifest = {
            "fixture": fixture.name,
            "product": str(product),
            "dataset_path": str(dataset_root),
            "run_root": str(run_root),
            "results_path": str(run_root / "results.json"),
            "trace_path": str(run_root / "trace.json"),
            "s3_prefix": f"{options.prefix_root}/{run_root.name}" if options else None,
            "results": [result.as_json() for result in results],
        }
        (workspace / "remote-result.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        return 0
    finally:
        if not args.keep_artifacts:
            cleanup_prefixes = []
            if run_root is not None and options is not None:
                cleanup_prefixes.append(f"{options.prefix_root}/{run_root.name}")
            elif options is not None and artifacts_root.is_dir():
                cleanup_prefixes.extend(
                    f"{options.prefix_root}/{path.name}"
                    for path in artifacts_root.iterdir()
                    if path.is_dir() and path not in before
                )
            for prefix in cleanup_prefixes:
                _cleanup_s3(settings, prefix)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, ValueError, zipfile.BadZipFile) as error:
        print(f"remote benchmark error: {error}", file=sys.stderr)
        raise SystemExit(1)
