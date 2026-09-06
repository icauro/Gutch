#!/usr/bin/env python3
"""Run isolated Gutch correctness scenarios against the real CLI."""
# Depends on Python, Cargo, and the real gutch-cli; S3 additionally uses test-owned Docker.
# Writes isolated runs under .artifacts/test/gutch. No benchmark imports or datasets.
# Run without arguments for Rust tests and a CLI smoke test; --suite all runs all correctness cases.
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from collections.abc import Callable
from datetime import datetime, timezone
import os
from pathlib import Path
import platform
import subprocess
import sys
import time
import traceback
import unittest
from uuid import uuid4

REPOSITORY = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPOSITORY))

from scripts._common.paths import get_default_cargo_target_dir
from tests.common._run_events import RunEventWriter
from gutch_repo.tests.support.cli import Case, execute
from gutch_repo.tests.support.filesystem import remove_owned, write_json
from gutch_repo.tests.workflows import capture, cleanup, failure_recovery, reconstruction, reference_format, remote_delivery, repair


def run_isolation(case):
    children = [Case(case.payload / name, case.executable) for name in ("first", "second")]
    with ThreadPoolExecutor(max_workers=2) as pool:
        list(pool.map(capture.capture_roundtrip, children))
    assert children[0].environment != children[1].environment
    assert all((child.payload / "repo").is_dir() for child in children)


@dataclass(frozen=True)
class Scenario:
    name: str
    operation: Callable[[Case], None]
    suite: str = "local"


SCENARIOS = [
    Scenario("capture_roundtrip", capture.capture_roundtrip, "fast"),
    Scenario("capture_deduplication", capture.capture_deduplication),
    Scenario("capture_profiles", capture.capture_profiles),
    Scenario("capture_metadata_conflict", capture.capture_metadata_conflict),
    Scenario("source_ingestion", capture.source_ingestion),
    Scenario("source_ingestion_auto", capture.source_ingestion_auto),
    Scenario("reference_format", reference_format.reference_format),
    Scenario("update_and_source_reuse", reconstruction.update_and_source_reuse),
    Scenario("filesystem_metadata", reconstruction.filesystem_metadata),
    Scenario("symbolic_links", reconstruction.symbolic_links),
    Scenario("verify_and_repair", repair.verify_and_repair),
    Scenario("repository_initialization", cleanup.repository_initialization),
    Scenario("snapshot_cleanup", cleanup.snapshot_cleanup),
    Scenario("recapture_missing_artifacts", cleanup.recapture_missing_artifacts),
    Scenario("staging_safety", failure_recovery.staging_safety),
    Scenario("staging_link_alias", failure_recovery.staging_link_alias),
    Scenario("unsafe_blueprint", failure_recovery.unsafe_blueprint),
    Scenario("failed_promotion", failure_recovery.failed_promotion),
    Scenario("http_delivery", remote_delivery.http_delivery),
    Scenario("failed_fetch_preserves_cache", remote_delivery.failed_fetch_preserves_cache),
    Scenario("transfer_failures_and_retry", remote_delivery.transfer_failures_and_retry),
    Scenario("interrupted_transfer", remote_delivery.interrupted_transfer),
    Scenario("run_isolation", run_isolation),
    Scenario("s3_delivery", remote_delivery.s3_delivery, "s3"),
]


def selected_scenarios(suite, names):
    if names:
        return [scenario for scenario in SCENARIOS if scenario.name in names]
    return [scenario for scenario in SCENARIOS if suite == "all" or scenario.suite == suite
            or (suite == "local" and scenario.suite == "fast")]


def main(arguments=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=("fast", "local", "s3", "all"), default="fast")
    parser.add_argument("--case", choices=[s.name for s in SCENARIOS], action="append")
    parser.add_argument("--list", action="store_true", help="List cases without building or allocating resources")
    parser.add_argument("--keep-artifacts", action="store_true", help="Retain successful payloads as well as failed ones")
    args = parser.parse_args(arguments)
    scenarios = selected_scenarios(args.suite, args.case)
    if args.list:
        for scenario in scenarios:
            print(f"{scenario.name}: {scenario.suite}")
        return 0
    root = REPOSITORY / ".artifacts/test/gutch" / f"{datetime.now(timezone.utc):%Y%m%dT%H%M%S}-{uuid4().hex[:6]}"
    root.mkdir(parents=True)
    print(f"Run: {root}", flush=True)
    results = []
    writer = RunEventWriter.from_environment("gutch")
    writer.phase_plan([(scenario.name, scenario.name) for scenario in scenarios])
    revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPOSITORY, capture_output=True, text=True, check=True).stdout.strip()
    manifest = {"schema_version": 1, "status": "running", "revision": revision,
                "platform": platform.platform(), "suite": args.suite, "cases": results}
    write_json(root / "run.json", manifest)
    try:
        target = get_default_cargo_target_dir(REPOSITORY)
        environment = {"CARGO_TARGET_DIR": str(target)}
        if args.suite in {"fast", "all"} and not args.case:
            execute(["cargo", "test", "--manifest-path", REPOSITORY / "gutch_repo/Cargo.toml"],
                    REPOSITORY, environment, root / "rust-tests.log", timeout=900)
        execute(["cargo", "build", "--manifest-path", REPOSITORY / "gutch_repo/gutch-cli/Cargo.toml"],
                REPOSITORY, environment, root / "build.log", timeout=900)
        executable = Path(target) / "debug" / ("gutch-cli.exe" if os.name == "nt" else "gutch-cli")
        assert executable.is_file(), f"Cargo did not produce {executable}"
        for index, scenario in enumerate(scenarios, 1):
            started = time.monotonic()
            case = Case(root / "cases" / f"{index:02d}-{scenario.name[:28]}", executable)
            result = {"name": scenario.name, "status": "running", "evidence": case.root.relative_to(root).as_posix()}
            results.append(result)
            writer.emit("phase_started", phase_id=scenario.name, phase_name=scenario.name)
            try:
                scenario.operation(case)
                if not args.keep_artifacts:
                    remove_owned(case.payload, case.root)
                result["status"] = "passed"
            except unittest.SkipTest as error:
                result.update(status="skipped", reason=str(error))
                if not args.keep_artifacts:
                    remove_owned(case.payload, case.root)
            except Exception as error:
                result.update(status="failed", reason=str(error) or f"{type(error).__name__}: see failure.txt")
                (case.root / "failure.txt").write_text(traceback.format_exc(), encoding="utf-8")
            result.update(seconds=round(time.monotonic() - started, 3), checks=case.checks)
            write_json(case.root / "results.json", result)
            write_json(root / "run.json", manifest)
            writer.emit("phase_finished", phase_id=scenario.name, phase_name=scenario.name, status=result["status"])
            print(f"[{result['status']}] {scenario.name} ({result['seconds']}s) {result.get('reason', '')}", flush=True)
        manifest["status"] = "failed" if any(r["status"] == "failed" for r in results) else "passed"
    except KeyboardInterrupt:
        manifest.update(status="cancelled", reason="interrupted")
        if results and results[-1]["status"] == "running":
            results[-1].update(status="cancelled", reason="interrupted")
            write_json(root / results[-1]["evidence"] / "results.json", results[-1])
    except Exception as error:
        manifest.update(status="failed", reason=str(error))
        (root / "failure.txt").write_text(traceback.format_exc(), encoding="utf-8")
    finally:
        write_json(root / "run.json", manifest)
        lines = ["# Gutch correctness", "", f"Status: {manifest['status']}", "",
                 "| Case | Status | Seconds | Evidence |", "| --- | --- | --- | --- |"]
        lines += [f"| {r['name']} | {r['status']} | {r.get('seconds', '')} | [{r['evidence']}]({r['evidence']}/results.json) |" for r in results]
        if manifest.get("reason"):
            lines += ["", manifest["reason"]]
        (root / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"{manifest['status']}: {root / 'summary.md'}", flush=True)
    return 0 if manifest["status"] == "passed" else 1


if __name__ == "__main__":
    from tests.common._test_environment import run_test_main
    raise SystemExit(run_test_main(main))
