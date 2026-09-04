"""Remote-side entry point for the fixed Linux Gutch benchmark."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import stat
import subprocess
import sys
import urllib.request
import zipfile


def _download_binary(url: str, expected_sha256: str, destination: Path) -> None:
    temporary = destination.with_name(f".{destination.name}.download")
    digest = hashlib.sha256()
    try:
        with urllib.request.urlopen(url, timeout=60) as response, temporary.open("wb") as output:
            while block := response.read(1024 * 1024):
                digest.update(block)
                output.write(block)
        actual_sha256 = digest.hexdigest()
        if actual_sha256 != expected_sha256:
            raise RuntimeError(
                f"hostapp-cli SHA256 mismatch: expected {expected_sha256}, got {actual_sha256}"
            )
        temporary.replace(destination)
        destination.chmod(destination.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    finally:
        temporary.unlink(missing_ok=True)


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
    from gutch_repo.tests.benchmarks.gutch_fixture_runner.report import print_fixture_report
    from gutch_repo.tests.benchmarks.gutch_fixture_runner.s3 import S3Settings
    from gutch_repo.tests.benchmarks.gutch_fixture_runner.s3_cycle import (
        load_s3_cycle,
        run_s3_cycle,
    )

    config = load_benchmark_config(args.config.resolve())
    load_benchmark_environment(args.config.resolve(), overwrite=True)
    binary_spec = config.hostapp_cli_linux_x64
    if binary_spec is None:
        raise RuntimeError("benchmark config requires binaries.hostapp_cli_linux_x64")

    binary_path = workspace / "bin" / "hostapp-cli"
    binary_path.parent.mkdir(parents=True, exist_ok=True)
    _download_binary(binary_spec.url, binary_spec.sha256, binary_path)
    version = subprocess.run(
        [str(binary_path), "--print-release-version"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if version != binary_spec.version:
        raise RuntimeError(
            f"hostapp-cli release version mismatch: expected {binary_spec.version}, got {version}"
        )

    dataset_root = Path(config.remote_fixtures_root) / workspace.name
    product = dataset_root / "Blender"
    recipe_path = code_root / "gutch_repo" / "tests" / "test_dataset_recipes" / "blender-official.json"
    if config.blender_platforms != ("linux-x64",):
        raise RuntimeError("remote Gutch benchmark requires dataset.blender_platforms: [linux-x64]")
    prepare_blender_product(
        recipe_path=recipe_path,
        product_dir=product,
        versions=config.blender_versions,
        platforms=config.blender_platforms,
        cache_dir=workspace / "blender-archives",
        force=True,
    )

    fixture_path = (
        code_root
        / "gutch_repo"
        / "tests"
        / "benchmarks"
        / "gutch_fixture_runner"
        / "fixtures"
        / f"{args.fixture}.yaml"
    )
    fixture, options = load_s3_cycle(fixture_path)
    # Perfetto is opened by the local orchestrator after the trace is downloaded.
    from dataclasses import replace

    fixture = replace(fixture, open_perfetto=False)
    artifacts_root = workspace / "artifacts"
    before = {path for path in artifacts_root.iterdir()} if artifacts_root.is_dir() else set()
    settings = S3Settings.from_environment("S3_MAIN")
    run_root: Path | None = None
    try:
        results = run_s3_cycle(
            fixture,
            product,
            GutchCli(binary_path),
            settings,
            artifacts_root,
            options=options,
            keep_artifacts=True,
        )
        run_root = _new_run_root(artifacts_root, before)
        print_fixture_report(fixture, product, results)

        manifest = {
            "fixture": fixture.name,
            "product": str(product),
            "run_root": str(run_root),
            "results_path": str(run_root / "results.json"),
            "trace_path": str(run_root / "trace.json"),
            "s3_prefix": f"{options.prefix_root}/{run_root.name}",
            "results": [result.as_json() for result in results],
        }
        (workspace / "remote-result.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        return 0
    finally:
        if not args.keep_artifacts:
            shutil.rmtree(dataset_root, ignore_errors=True)
            cleanup_prefixes = []
            if run_root is not None:
                cleanup_prefixes.append(f"{options.prefix_root}/{run_root.name}")
            elif artifacts_root.is_dir():
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
