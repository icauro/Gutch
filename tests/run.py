#!/usr/bin/env python3
"""Run isolated Gutch correctness scenarios against the real CLI."""

# Depends on Python, Cargo, and the real gutch-cli; S3 additionally uses test-owned Docker.
# Writes isolated runs under tests/.artifacts/runs/gutch. No benchmark imports or datasets.
# Default: tooling, Rust and all local scenarios. Docker is explicit via --suite s3/all.
from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import os
from pathlib import Path
import platform
import sys
import traceback
from uuid import uuid4

REPOSITORY = Path(__file__).resolve().parents[1]
if __package__ in {None, ""}:
    sys.path.insert(0, str(REPOSITORY))
    __package__ = "tests"

from .helpers.events import RunEventWriter
from .helpers.cli import Case, execute
from .helpers.filesystem import write_json
from .helpers.scenario_process import run_scenario
from .helpers.reproduction import source_revision
from tools.artifacts import compact_success, prune_successful_runs


@dataclass(frozen=True)
class Scenario:
    module: str
    suite: str = "local"

    @property
    def name(self):
        return self.module.replace(".", "/")

    @property
    def category(self):
        return self.module.split(".")[0]


SCENARIOS = [
    Scenario("repository.tag_constraints_preserve_state", "local"),
    Scenario("repository.snapshot_diff_reports_tree_changes", "local"),
    Scenario("repository.command_contract", "local"),
    Scenario("repository.filename_constraints_reject_without_changes", "local"),
    Scenario("repository.lifecycle", "local"),
    Scenario("capture.profiling_preserves_content", "local"),
    Scenario("repository.concurrent_writers_preserve_snapshots", "local"),
    Scenario("repository.index_rebuild_verifies_before_publication", "local"),
    Scenario("capture.batch_failure_preserves_completed_snapshots", "local"),
    Scenario("repository.verification_detects_missing_artifacts", "local"),
    Scenario("reconstruction.comparison_detects_mismatches", "local"),
    Scenario("capture.repeated_capture_preserves_identity", "fast"),
    Scenario("capture.changed_versions_reuse_chunks", "local"),
    Scenario("capture.storage_profiles_preserve_content", "local"),
    Scenario("capture.conflicting_tags_preserve_metadata", "local"),
    Scenario("capture.explicit_discovery_uses_declared_sources", "local"),
    Scenario("capture.automatic_discovery_respects_exclusions", "local"),
    Scenario("repository.frozen_snapshots_match_inventories", "local"),
    Scenario("reconstruction.updates_reuse_source_chunks", "local"),
    Scenario("reconstruction.metadata_and_long_paths_survive", "local"),
    Scenario("reconstruction.symlinks_and_attributes_survive", "local"),
    Scenario("repair.damaged_files_recover_without_losing_extras", "local"),
    Scenario("repository.initialization_preserves_unrelated_files", "local"),
    Scenario("repository.deletion_preserves_shared_artifacts", "local"),
    Scenario("repository.recapture_recovers_missing_artifacts", "local"),
    Scenario("reconstruction.overlapping_staging_preserves_files", "local"),
    Scenario("reconstruction.staging_symlinks_cannot_bypass_overlap_checks", "local"),
    Scenario("reconstruction.unsafe_paths_leave_files_unchanged", "local"),
    Scenario("reconstruction.failed_publication_preserves_installation", "local"),
    Scenario("delivery.http_reuses_cached_artifacts", "local"),
    Scenario("delivery.invalid_metadata_preserves_cache", "local"),
    Scenario("delivery.failed_downloads_preserve_installation", "local"),
    Scenario("delivery.interrupted_download_can_retry", "local"),
    Scenario("capture.concurrent_runs_are_isolated", "local"),
    Scenario("delivery.s3_fetch_reconstruct_and_repair", "s3"),
]


def selected_scenarios(suite, names):
    if names:
        return [scenario for scenario in SCENARIOS if scenario.name in names]
    return [
        scenario
        for scenario in SCENARIOS
        if suite == "all"
        or scenario.suite == suite
        or (suite == scenario.category and scenario.suite != "s3")
        or (suite in {"check", "local"} and scenario.suite in {"fast", "local"})
    ]


def main(arguments=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--suite",
        choices=(
            "check",
            "fast",
            "local",
            "s3",
            "all",
            "capture",
            "repository",
            "reconstruction",
            "delivery",
            "repair",
        ),
        default="check",
        help="check: tooling, Rust and local scenarios; s3/all require Docker",
    )
    parser.add_argument("--case", choices=[s.name for s in SCENARIOS], action="append")
    parser.add_argument(
        "--repo",
        type=Path,
        help="Run the core lifecycle on an isolated copy of this repository",
    )
    from tests.datasets.generate import case_names, GENERATOR_VERSION

    parser.add_argument(
        "--dataset",
        choices=case_names(),
        help="Run the core lifecycle on a corpus case (Blender downloads on demand)",
    )
    parser.add_argument(
        "--repository-version",
        default="v1",
        help="Repository recipe version (default: v1; synthetic cases use frozen data)",
    )
    parser.add_argument(
        "--expected",
        type=Path,
        help="Independent snapshot inventories for --repo (snapshot ID -> entries)",
    )
    parser.add_argument(
        "--seed",
        type=int,
        help="Run the core lifecycle with this deterministic dataset/sequence seed",
    )
    parser.add_argument(
        "--sequence",
        help="Replay explicit additional operations: capture,rebuild,repair,reject,delete",
    )
    parser.add_argument(
        "--steps",
        type=int,
        default=4,
        help="Additional seeded lifecycle operations (default: 4)",
    )
    parser.add_argument(
        "--storage", choices=("bundled", "loose_chunks"), default="bundled"
    )
    parser.add_argument(
        "--compression", choices=("none", "lz4", "zstd"), default="zstd"
    )
    parser.add_argument(
        "--mutations",
        type=int,
        default=0,
        help="Seeded binary-parser mutations after the lifecycle",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=60,
        help="Core lifecycle deadline in seconds; other scenarios keep 10s",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="List cases without building or allocating resources",
    )
    parser.add_argument(
        "--keep-artifacts",
        action="store_true",
        help="Retain successful payloads as well as failed ones",
    )
    parser.add_argument(
        "--artifacts-root", type=Path, default=REPOSITORY / "tests/.artifacts"
    )
    parser.add_argument("--event-file", type=Path)
    parser.add_argument("--run-id")
    parser.add_argument("--suite-id", default="gutch")
    args = parser.parse_args(arguments)
    if args.steps < 0 or args.mutations < 0 or args.timeout < 1:
        parser.error("steps/mutations must be nonnegative and timeout must be positive")
    if args.sequence and any(
        name not in {"capture", "rebuild", "repair", "reject", "delete"}
        for name in args.sequence.split(",")
    ):
        parser.error("unknown lifecycle operation in --sequence")
    if args.expected and not args.repo:
        parser.error("--expected requires --repo")
    if args.dataset and (args.repo or args.expected):
        parser.error("--dataset supplies its own repository and expected inventories")
    if (
        args.dataset
        or args.repo
        or (args.seed is not None or args.sequence is not None)
        or args.mutations
    ):
        if args.case and args.case != ["repository/lifecycle"]:
            parser.error(
                "repository options select repository/lifecycle; remove other --case selections"
            )
        args.case = ["repository/lifecycle"]
    scenarios = selected_scenarios(args.suite, args.case)
    if args.list:
        for scenario in scenarios:
            print(f"{scenario.name}: {scenario.suite}")
        return 0
    root = (
        args.artifacts_root.resolve()
        / "runs/gutch"
        / f"{datetime.now(timezone.utc):%Y%m%dT%H%M%S}-{uuid4().hex[:6]}"
    )
    root.mkdir(parents=True)
    print(f"Run: {root}", flush=True)
    results = []
    writer = RunEventWriter(args.event_file, args.run_id or root.name, args.suite_id)
    writer.phase_plan([(scenario.name, scenario.name) for scenario in scenarios])
    revision = source_revision()
    manifest = {
        "schema_version": 1,
        "status": "running",
        "revision": revision,
        "platform": platform.platform(),
        "suite": args.suite,
        "rust_tests": "not requested",
        "cases": results,
        "keep_artifacts": args.keep_artifacts,
        "cleanup_complete": False,
    }
    write_json(root / "run.json", manifest)
    services = None
    try:
        target = REPOSITORY / ".builds/compiler-cache/cargo"
        environment = {"CARGO_TARGET_DIR": str(target)}
        if args.suite in {"check", "fast", "all"} and not args.case:
            execute(
                [
                    sys.executable,
                    "-m",
                    "pytest",
                    "tests/harness_tests",
                    "-q",
                    "--basetemp",
                    root / "tooling-temp",
                    "-o",
                    f"cache_dir={root / 'tooling-cache'}",
                ],
                REPOSITORY,
                environment,
                root / "tooling-tests.log",
                timeout=120,
            )
            manifest["rust_tests"] = "running"
            execute(
                [
                    "cargo",
                    "test",
                    "--locked",
                    "--workspace",
                    "--manifest-path",
                    REPOSITORY / "Cargo.toml",
                ],
                REPOSITORY,
                environment,
                root / "rust-tests.log",
                timeout=900,
            )
            manifest["rust_tests"] = "passed"
        execute(
            [
                "cargo",
                "build",
                "--locked",
                "--manifest-path",
                REPOSITORY / "gutch-cli/Cargo.toml",
            ],
            REPOSITORY,
            environment,
            root / "build.log",
            timeout=900,
        )
        executable = (
            Path(target)
            / "debug"
            / ("gutch-cli.exe" if os.name == "nt" else "gutch-cli")
        )
        assert executable.is_file(), f"Cargo did not produce {executable}"
        scenario_environment = {}
        if any(s.suite == "s3" for s in scenarios):
            from tools.services import Services

            services = Services()
            services.start()
            scenario_environment = {
                "GUTCH_TEST_S3_ENDPOINT": services.endpoint,
                "GUTCH_TEST_S3_ACCESS_KEY": "gutch-test-access",
                "GUTCH_TEST_S3_SECRET_KEY": "gutch-test-secret",
                "GUTCH_TEST_S3_PROJECT": services.project,
            }
        for index, scenario in enumerate(scenarios, 1):
            print(f"[{index}/{len(scenarios)}] Starting {scenario.name}", flush=True)
            case = Case(root / "cases" / scenario.name, executable)
            options = {
                "seed": args.seed
                if args.seed is not None
                else int.from_bytes(os.urandom(8), "big"),
                "steps": args.steps,
                "storage": args.storage,
                "compression": args.compression,
                "mutations": args.mutations,
                "timeout": args.timeout,
                "generator_version": GENERATOR_VERSION,
            }
            if args.sequence is not None:
                options["sequence"] = args.sequence
            if args.repo:
                options["repo"] = str(args.repo.resolve())
            if args.expected:
                options["expected"] = str(args.expected.resolve())
            if args.dataset:
                options.update(
                    dataset=args.dataset, repository_version=args.repository_version
                )
                from tests.datasets.generate import load_case
                from tests.helpers.corpus import repository_input

                record = load_case(args.dataset)["repositories"].get(
                    args.repository_version, {}
                )
                if record.get("preparation") == "capture":
                    # Network preparation belongs to setup, outside the scenario
                    # deadline. The worker and replay use the same prepared input.
                    source, expected = repository_input(
                        args.dataset, args.repository_version, case=case
                    )
                    expected_path = case.root / "dataset-expected.json"
                    write_json(expected_path, expected)
                    options.update(repo=str(source), expected=str(expected_path))
            write_json(case.root / "options.json", options)
            write_json(
                case.root / "replay.json",
                {
                    "schema_version": 1,
                    "case": scenario.name,
                    "revision": revision,
                    "platform": platform.platform(),
                    "options": options
                    if scenario.name == "repository/lifecycle"
                    else {},
                },
            )
            result = {
                "name": scenario.name,
                "status": "running",
                "evidence": case.root.relative_to(root).as_posix(),
            }
            results.append(result)
            writer.emit(
                "phase_started", phase_id=scenario.name, phase_name=scenario.name
            )
            result.update(
                run_scenario(
                    f"tests.scenarios.{scenario.module}",
                    case,
                    scenario_environment,
                    keep_artifacts=args.keep_artifacts,
                    timeout=args.timeout
                    if scenario.name
                    in {
                        "repository/lifecycle",
                        "repository/frozen_snapshots_match_inventories",
                    }
                    else 10,
                )
            )
            write_json(case.root / "results.json", result)
            write_json(root / "run.json", manifest)
            writer.emit(
                "phase_finished",
                phase_id=scenario.name,
                phase_name=scenario.name,
                status=result["status"],
            )
            print(
                f"[{result['status']}] {scenario.name} ({result['seconds']}s) {result.get('reason', '')}",
                flush=True,
            )
            if result.get("details", {}).get("oracle"):
                print(
                    f"  Validation: {result['details']['oracle']}; seed={result['details']['seed']}",
                    flush=True,
                )
            if result["status"] == "failed":
                print(
                    f'Evidence: {case.root}\nReplay: python gutch.py replay "{case.root}"',
                    flush=True,
                )
        manifest["status"] = (
            "failed" if any(r["status"] == "failed" for r in results) else "passed"
        )
    except KeyboardInterrupt:
        manifest.update(status="cancelled", reason="interrupted")
        if results and results[-1]["status"] == "running":
            results[-1].update(status="cancelled", reason="interrupted")
            write_json(root / results[-1]["evidence"] / "results.json", results[-1])
    except Exception as error:
        manifest.update(status="failed", reason=str(error))
        (root / "failure.txt").write_text(traceback.format_exc(), encoding="utf-8")
    finally:
        if services is not None:
            try:
                services.close()
            except Exception as error:
                manifest.update(
                    status="failed", reason=f"Service cleanup failed: {error}"
                )
        if manifest["rust_tests"] == "running":
            manifest["rust_tests"] = manifest["status"]
        write_json(root / "run.json", manifest)
        counts = {
            status: sum(result["status"] == status for result in results)
            for status in ("passed", "failed", "skipped", "cancelled")
        }
        totals = ", ".join(f"{count} {status}" for status, count in counts.items())
        totals += f", {len(scenarios) - len(results)} not run"
        summary = [
            f"Status: {manifest['status']}",
            f"Rust tests: {manifest['rust_tests']}",
            f"CLI scenarios: {totals}",
        ]
        lines = [
            "# Gutch correctness",
            "",
            *summary,
            "",
            "| Case | Status | Seconds | Evidence |",
            "| --- | --- | --- | --- |",
        ]
        lines += [
            f"| {r['name']} | {r['status']} | {r.get('seconds', '')} | [{r['evidence']}]({r['evidence']}/results.json) |"
            for r in results
        ]
        if manifest.get("reason"):
            lines += ["", manifest["reason"]]
        (root / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
        if manifest["status"] == "passed":
            try:
                if not args.keep_artifacts:
                    # The summary must not link to detailed evidence that is about to be removed.
                    (root / "summary.md").write_text(
                        "\n".join(
                            [
                                "# Gutch correctness",
                                "",
                                *summary,
                                "",
                                *[
                                    f"- {r['name']}: {r['status']} ({r.get('seconds', '')}s) {r.get('details', {}).get('oracle', '')}"
                                    for r in results
                                ],
                            ]
                        )
                        + "\n",
                        encoding="utf-8",
                    )
                    for result in results:
                        result.pop("evidence", None)
                compact_success(root, keep=args.keep_artifacts)
                manifest["cleanup_complete"] = True
                write_json(root / "run.json", manifest)
                prune_successful_runs(root.parent)
            except Exception as error:
                manifest.update(
                    status="failed", reason=f"Artifact cleanup failed: {error}"
                )
                write_json(root / "run.json", manifest)
                summary[0] = "Status: failed"
                (root / "summary.md").write_text(
                    "\n".join(
                        ["# Gutch correctness", "", *summary, "", manifest["reason"]]
                    )
                    + "\n",
                    encoding="utf-8",
                )
    print("\nGutch correctness summary\n" + "\n".join(summary), flush=True)
    if manifest.get("reason"):
        print(manifest["reason"], flush=True)
    print(f"Report: {root / 'summary.md'}", flush=True)
    return 0 if manifest["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
