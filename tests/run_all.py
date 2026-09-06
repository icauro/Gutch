#!/usr/bin/env python3
"""Run isolated Gutch correctness scenarios against the real CLI."""
# Depends on Python, Cargo, and the real gutch-cli; S3 additionally uses test-owned Docker.
# Writes isolated runs under .artifacts/test/gutch. No benchmark imports or datasets.
# Run without arguments for every correctness case, including S3; --suite fast runs a quick check.
from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import os
from pathlib import Path
import platform
import subprocess
import sys
import traceback
from uuid import uuid4

REPOSITORY = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPOSITORY))

from scripts._common.paths import get_default_cargo_target_dir
from tests.common._run_events import RunEventWriter
from gutch_repo.tests.support.cli import Case, execute
from gutch_repo.tests.support.filesystem import write_json
from gutch_repo.tests.support.scenario_process import run_scenario
from gutch_repo.tests.support.delivery import prepare_s3_environment
@dataclass(frozen=True)
class Scenario:
    name: str
    module: str
    suite: str = "local"


SCENARIOS = [
    Scenario("capture_roundtrip", "repeated_capture_preserves_snapshot_identity_and_roundtrips_files", "fast"),
    Scenario("capture_deduplication", "capturing_changed_versions_reuses_chunks_and_restores_both_versions", "local"),
    Scenario("capture_profiles", "compression_and_storage_profiles_preserve_file_contents", "local"),
    Scenario("capture_metadata_conflict", "conflicting_capture_tags_leave_existing_metadata_unchanged", "local"),
    Scenario("source_ingestion", "explicit_source_discovery_uses_declared_paths_and_tags_only", "local"),
    Scenario("source_ingestion_auto", "automatic_source_discovery_creates_config_and_ignores_excluded_folders", "local"),
    Scenario("reference_format", "frozen_repository_snapshots_reconstruct_to_independent_expected_inventories", "local"),
    Scenario("update_and_source_reuse", "successive_installation_updates_reuse_source_chunks_and_replace_old_content", "local"),
    Scenario("filesystem_metadata", "reconstruction_preserves_permissions_empty_directories_and_long_paths", "local"),
    Scenario("symbolic_links", "reconstruction_preserves_symbolic_link_targets_and_file_attributes", "local"),
    Scenario("verify_and_repair", "repair_restores_damaged_files_and_preserves_user_added_files_unless_requested", "local"),
    Scenario("repository_initialization", "repository_initialization_preserves_unrelated_files_and_rejects_reinitialization", "local"),
    Scenario("snapshot_cleanup", "snapshot_deletion_preserves_shared_artifacts_and_rejects_incomplete_indexes", "local"),
    Scenario("recapture_missing_artifacts", "recapture_rebuilds_missing_artifacts_for_identical_and_changed_snapshots", "local"),
    Scenario("staging_safety", "overlapping_staging_paths_fail_without_modifying_installed_or_unrelated_files", "local"),
    Scenario("staging_link_alias", "symbolic_link_staging_alias_cannot_bypass_destination_overlap_checks", "local"),
    Scenario("unsafe_blueprint", "unsafe_blueprint_paths_are_rejected_without_filesystem_mutation", "local"),
    Scenario("failed_promotion", "blocked_installation_promotion_preserves_old_content_and_allows_retry", "local"),
    Scenario("http_delivery", "http_delivery_reuses_cached_artifacts_and_supports_combined_reconstruction", "local"),
    Scenario("failed_fetch_preserves_cache", "invalid_downloaded_metadata_preserves_the_existing_cached_snapshot", "local"),
    Scenario("transfer_failures_and_retry", "failed_and_transient_downloads_preserve_installations_and_succeed_on_retry", "local"),
    Scenario("interrupted_transfer", "interrupted_download_preserves_installation_and_succeeds_on_retry", "local"),
    Scenario("run_isolation", "concurrent_captures_use_independent_repositories_and_configuration", "local"),
    Scenario("s3_delivery", "s3_delivery_supports_cached_fetch_combined_reconstruction_and_repair", "s3"),
]


def selected_scenarios(suite, names):
    if names:
        return [scenario for scenario in SCENARIOS if scenario.name in names]
    return [scenario for scenario in SCENARIOS if suite == "all" or scenario.suite == suite
            or (suite == "local" and scenario.suite == "fast")]


def main(arguments=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=("fast", "local", "s3", "all"), default="all")
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
                "platform": platform.platform(), "suite": args.suite, "rust_tests": "not requested", "cases": results}
    write_json(root / "run.json", manifest)
    try:
        target = get_default_cargo_target_dir(REPOSITORY)
        environment = {"CARGO_TARGET_DIR": str(target)}
        if args.suite in {"fast", "all"} and not args.case:
            manifest["rust_tests"] = "running"
            execute(["cargo", "test", "--manifest-path", REPOSITORY / "gutch_repo/Cargo.toml"],
                    REPOSITORY, environment, root / "rust-tests.log", timeout=900)
            manifest["rust_tests"] = "passed"
        execute(["cargo", "build", "--manifest-path", REPOSITORY / "gutch_repo/gutch-cli/Cargo.toml"],
                REPOSITORY, environment, root / "build.log", timeout=900)
        executable = Path(target) / "debug" / ("gutch-cli.exe" if os.name == "nt" else "gutch-cli")
        assert executable.is_file(), f"Cargo did not produce {executable}"
        scenario_environment = prepare_s3_environment() if any(s.suite == "s3" for s in scenarios) else {}
        for index, scenario in enumerate(scenarios, 1):
            case = Case(root / "cases" / f"{index:02d}-{scenario.name[:28]}", executable)
            result = {"name": scenario.name, "status": "running", "evidence": case.root.relative_to(root).as_posix()}
            results.append(result)
            writer.emit("phase_started", phase_id=scenario.name, phase_name=scenario.name)
            result.update(run_scenario(f"gutch_repo.tests.workflows.{scenario.module}", case, scenario_environment,
                                       keep_artifacts=args.keep_artifacts))
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
        if manifest["rust_tests"] == "running":
            manifest["rust_tests"] = manifest["status"]
        write_json(root / "run.json", manifest)
        counts = {status: sum(result["status"] == status for result in results)
                  for status in ("passed", "failed", "skipped", "cancelled")}
        totals = ", ".join(f"{count} {status}" for status, count in counts.items())
        totals += f", {len(scenarios) - len(results)} not run"
        summary = [f"Status: {manifest['status']}", f"Rust tests: {manifest['rust_tests']}",
                   f"CLI scenarios: {totals}"]
        lines = ["# Gutch correctness", "", *summary, "",
                 "| Case | Status | Seconds | Evidence |", "| --- | --- | --- | --- |"]
        lines += [f"| {r['name']} | {r['status']} | {r.get('seconds', '')} | [{r['evidence']}]({r['evidence']}/results.json) |" for r in results]
        if manifest.get("reason"):
            lines += ["", manifest["reason"]]
        (root / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\nGutch correctness summary\n" + "\n".join(summary), flush=True)
    if manifest.get("reason"):
        print(manifest["reason"], flush=True)
    print(f"Report: {root / 'summary.md'}", flush=True)
    return 0 if manifest["status"] == "passed" else 1


if __name__ == "__main__":
    from tests.common._test_environment import run_test_main
    raise SystemExit(run_test_main(main))
