#!/usr/bin/env python3
"""Run the deterministic Gutch regression through a temporary local S3 bucket."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import sys
from urllib.parse import urlsplit

REPO_ROOT = Path(__file__).resolve().parents[2]
if __package__ in {None, ""}:
    sys.path.insert(0, str(REPO_ROOT))

from gutch_repo.test_benchmarks.gutch_fixture_runner.commands import GutchCli
from gutch_repo.test_benchmarks.gutch_fixture_runner.reconstruction_storage import run_reconstruction_storage
from gutch_repo.test_benchmarks.gutch_fixture_runner.s3 import S3Settings, temporary_bucket
from gutch_repo.test_regressions.run_all import (
    build_release_gutch_cli,
    materialize_regression_product,
    select_latest_snapshots,
)

S3_ARTIFACT_ROOT = REPO_ROOT / ".artifacts" / "s3"


def _local_s3_settings() -> S3Settings:
    from tests.common._e2e_harness import (
        create_runtime_context, load_backend_env_values, ensure_docker_engine_ready,
        compose_up, wait_for_compose_service_health,
    )
    values, env_path = load_backend_env_values(REPO_ROOT)
    context = create_runtime_context(
        REPO_ROOT, lane_name="gutch-s3", project_name="",
        backend_values=values, backend_env_file_path=env_path,
        runtime_mode=os.environ.get("HOSTAPP_TEST_RUNTIME_MODE", "fast"),
    )
    ensure_docker_engine_ready(REPO_ROOT)
    compose_up(context.compose_file, context.project_name, context.env_file,
               services=["seaweedfs"], build=False)
    wait_for_compose_service_health(context.compose_file, context.project_name,
                                   context.env_file, "seaweedfs")
    return S3Settings(
        bucket="", endpoint=context.backend_values["S3_PUBLIC_BASE_URL"].removesuffix("/hostapp-public"),
        region="us-east-1",
        access_key_id=context.backend_values["S3_MAIN_ACCESS_KEY"],
        secret_access_key=context.backend_values["S3_MAIN_SECRET_KEY"],
    )


def _assert_regression_passed(result: dict) -> None:
    differences = result.get("differences", {})
    failures = {name: entries for name, entries in differences.items() if entries}
    if failures:
        raise RuntimeError(f"S3 regression produced reconstructed differences: {failures}")
    if not result.get("verification", {}).get("corrupt_repair_after_reconstruct"):
        raise RuntimeError("S3 regression did not execute corruption and repair")


def main(arguments: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--keep-artifacts",
        action="store_true",
        help="keep generated S3 regression payloads under .artifacts/s3",
    )
    args = parser.parse_args(arguments)

    try:
        settings = _local_s3_settings()
        product_root = materialize_regression_product()
        snapshots = select_latest_snapshots(product_root)
        product = snapshots[-1][1]
        print(f"Using generated snapshot {product.name} from {product_root}", flush=True)
        executable = build_release_gutch_cli()
        with temporary_bucket(settings, retain_on_failure=True) as bucket_settings:
            print(f"Created temporary local S3 bucket {bucket_settings.bucket}", flush=True)
            result = run_reconstruction_storage(
                product,
                GutchCli(executable),
                bucket_settings,
                S3_ARTIFACT_ROOT,
                recovery_enabled=True,
                keep_artifacts=args.keep_artifacts,
                step_wait_seconds=0.0,
                run_label="s3",
                include_fetch_reconstruct=False,
            )
            _assert_regression_passed(result)
            print("S3 regression passed: capture, reconstruct, corrupt, repair, and compare", flush=True)
        print("Deleted temporary local S3 bucket", flush=True)
        return 0
    except (OSError, RuntimeError, subprocess.CalledProcessError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr, flush=True)
        return 1
    except KeyboardInterrupt:
        print("\nAborted.", file=sys.stderr, flush=True)
        return 1


if __name__ == "__main__":
    from tests.common._test_environment import run_test_main
    raise SystemExit(run_test_main(main))
