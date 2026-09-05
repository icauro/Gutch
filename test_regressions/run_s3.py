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

from gutch_repo.benchmarks.gutch_fixture_runner.commands import GutchCli
from gutch_repo.benchmarks.gutch_fixture_runner.reconstruction_storage import run_reconstruction_storage
from gutch_repo.benchmarks.gutch_fixture_runner.s3 import S3Settings, temporary_bucket
from gutch_repo.test_regressions.run_all import (
    build_release_hostapp_cli,
    materialize_regression_product,
    select_latest_snapshots,
)

S3_ARTIFACT_ROOT = REPO_ROOT / ".artifacts" / "s3"


def _ensure_local_s3() -> None:
    command = [
        sys.executable,
        str(REPO_ROOT / "scripts" / "dev" / "stack_up.py"),
        "--services",
        "seaweedfs",
        "--no-build",
    ]
    print(f"[ensure local S3] {' '.join(command)}", flush=True)
    subprocess.run(command, cwd=REPO_ROOT, check=True)


def _local_s3_settings() -> S3Settings:
    endpoint = os.environ.get("S3_MAIN_ENDPOINT", "http://127.0.0.1:9634").strip().rstrip("/")
    parsed = urlsplit(endpoint)
    if parsed.scheme not in {"http", "https"} or parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
        raise RuntimeError(
            "S3 regression requires the local dev SeaweedFS endpoint; "
            f"got {endpoint!r}"
        )
    return S3Settings(
        bucket="",
        endpoint=endpoint,
        region=os.environ.get("S3_MAIN_REGION", "auto").strip() or "auto",
        access_key_id=os.environ.get("S3_MAIN_ACCESS_KEY", "hostapp-local").strip(),
        secret_access_key=os.environ.get("S3_MAIN_SECRET_KEY", "hostapp-local-secret-key").strip(),
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
        _ensure_local_s3()
        settings = _local_s3_settings()
        product_root = materialize_regression_product()
        snapshots = select_latest_snapshots(product_root)
        product = snapshots[-1][1]
        print(f"Using generated snapshot {product.name} from {product_root}", flush=True)
        executable = build_release_hostapp_cli()
        with temporary_bucket(settings) as bucket_settings:
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
    raise SystemExit(main())
