"""Local Gutch fixture execution.

The workflow deliberately treats reconstruct verification as part of the
reconstruct command.  It reads the embedded verification timing from the
reconstruct stats and never starts a second snapshot-verify command.
"""

from dataclasses import dataclass
import json
from pathlib import Path
import re
import shutil
import time
from collections.abc import Callable
from typing import Any

from gutch_repo.tests.benchmarks.common.artifacts import RunLayout, create_run_dir
from gutch_repo.tests.benchmarks.common.command import CommandRunner
from gutch_repo.tests.benchmarks.common.filesystem import compare_directories, remove_tree, scan_tree
from gutch_repo.tests.benchmarks.common.recovery import apply_corruption_plan, build_corruption_plan
from gutch_repo.tests.benchmarks.common.traces import combine_labeled_traces, open_trace_in_perfetto
from .commands import GutchCli
from .external import ExternalTools, run_external
from .models import FixtureSpec, GutchProfile, LaneResult, LaneSpec


def _minimal_schema(path: Path) -> Path:
    path.write_text(
        json.dumps(
            {
                "tag_definitions": [
                    {
                        "tag_name": "version",
                        "tag_kind": "exclusive",
                        "is_required": True,
                        "inference_prefixes": ["-v", "-"],
                        "allowed_values": [],
                        "inference_strategy": "version",
                    },
                    {
                        "tag_name": "access",
                        "tag_kind": "exclusive",
                        "is_required": False,
                        "inference_prefixes": ["-"],
                        "allowed_values": ["public", "supporter"],
                        "inference_strategy": "default",
                    },
                ]
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    return path


def _version(name: str) -> str:
    match = re.search(r"(?<!\d)(\d+\.\d+\.\d+(?:\.\d+)?)(?!\d)", name)
    if match is None:
        raise ValueError(f"target {name!r} must contain a semantic version")
    return match.group(1)


def _stats(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def _snapshot_id(stats: dict[str, Any]) -> str:
    value = stats.get("snapshot_id")
    if not isinstance(value, str) or not value:
        raise ValueError("capture stats did not contain snapshot_id")
    return value


def _verify_seconds(stats: dict[str, Any]) -> float:
    timings = stats.get("verify_timings")
    if not isinstance(timings, dict):
        return 0.0
    value = timings.get("total_seconds")
    return float(value) if isinstance(value, (int, float)) else 0.0


@dataclass
class LocalGutchFixtureRunner:
    cli: GutchCli
    commands: CommandRunner
    run: RunLayout
    external: ExternalTools | None = None

    def _init_repo(self, lane_root: Path, repo: Path, label: str) -> None:
        schema = _minimal_schema(lane_root / "benchmark-schema.json")
        self.commands.run(
            self.cli.command("repo", "init", "--path", repo, "--schema", schema),
            description=f"{label} repository init",
        )

    def _capture(
        self,
        lane_root: Path,
        repo: Path,
        source: Path,
        profile: GutchProfile,
        stats_path: Path,
        trace_path: Path,
        tag: str,
        version_name: str | None = None,
    ) -> tuple[float, dict[str, Any]]:
        result = self.commands.run(
            self.cli.command(
                "snapshot",
                "capture",
                "--source-dir",
                source,
                "--repo",
                repo,
                "--yes",
                "--tag",
                f"version::{_version(version_name or source.name)}",
                "--tag",
                f"benchmark::{tag}",
                *GutchCli.profile_args(profile),
                "--stats-json-out",
                stats_path,
                "--trace-out",
                trace_path,
            ),
            description=f"{tag} capture {source.name}",
        )
        return result.elapsed_seconds, _stats(stats_path)

    def _capture_many(
        self,
        repo: Path,
        source_root: Path,
        profile: GutchProfile,
        stats_path: Path,
        trace_path: Path,
        tag: str,
        label: str,
    ) -> tuple[float, dict[str, Any]]:
        result = self.commands.run(
            self.cli.command(
                "snapshot",
                "capture",
                "--source-root",
                source_root,
                "--repo",
                repo,
                "--yes",
                "--tag",
                f"benchmark::{tag}",
                *GutchCli.profile_args(profile),
                "--stats-json-out",
                stats_path,
                "--trace-out",
                trace_path,
            ),
            description=f"capture-many {label}",
        )
        return result.elapsed_seconds, _stats(stats_path)

    def _reconstruct(
        self,
        repo: Path,
        snapshot_id: str,
        destination: Path,
        profile: GutchProfile,
        stats_path: Path,
        trace_path: Path,
        label: str,
    ) -> tuple[float, dict[str, Any]]:
        result = self.commands.run(
            self.cli.command(
                "snapshot",
                "reconstruct",
                "--repo",
                repo,
                "--snapshot-id",
                snapshot_id,
                "--destination",
                destination,
                *GutchCli.runtime_args(profile),
                "--stats-json-out",
                stats_path,
                "--trace-out",
                trace_path,
            ),
            description=f"reconstruct {label}",
        )
        return result.elapsed_seconds, _stats(stats_path)

    def _run_corrupt_repair(
        self,
        lane: Any,
        repo: Path,
        snapshots: list[tuple[str, str, Path]],
        profile: GutchProfile,
        label: str,
    ) -> dict[str, float | list[str]]:
        corrupt_seconds = 0.0
        repair_seconds = 0.0
        corrupt_traces: list[str] = []
        repair_traces: list[str] = []
        plans = []
        for index, (name, snapshot_id, destination) in enumerate(snapshots, start=1):
            plan = build_corruption_plan(
                destination,
                lane.stats / f"corruption-{index:03d}.json",
                index,
            )
            plans.append((name, snapshot_id, destination, plan))
        for index, (name, _snapshot_id, destination, plan) in enumerate(plans, start=1):
            stats = lane.stats / f"corrupt-{index:03d}.json"
            trace = lane.traces / f"corrupt-{index:03d}.json"
            result = self.commands.run(
                self.cli.command(
                    "reconstructed",
                    "corrupt",
                    "--destination",
                    destination,
                    "--plan",
                    plan,
                    "--stats-json-out",
                    stats,
                    "--yes",
                    "--trace-out",
                    trace,
                ),
                description=f"{label} corrupt {name}",
            )
            corrupt_seconds += result.elapsed_seconds
            corrupt_traces.append(str(trace))
        for index, (name, snapshot_id, destination, _plan) in enumerate(plans, start=1):
            stats = lane.stats / f"repair-{index:03d}.json"
            trace = lane.traces / f"repair-{index:03d}.json"
            result = self.commands.run(
                self.cli.command(
                    "reconstructed",
                    "repair",
                    "--repo",
                    repo,
                    "--snapshot-id",
                    snapshot_id,
                    "--destination",
                    destination,
                    "--remove-added-files",
                    "--yes",
                    *GutchCli.runtime_args(profile),
                    "--stats-json-out",
                    stats,
                    "--trace-out",
                    trace,
                ),
                description=f"{label} repair {name}",
            )
            repair_seconds += result.elapsed_seconds
            repair_traces.append(str(trace))
        return {
            "corrupt": corrupt_seconds,
            "repair": repair_seconds,
            "corrupt_traces": corrupt_traces,
            "repair_traces": repair_traces,
        }

    def _run_individual(
        self,
        lane: Any,
        product: Path,
        lane_spec: LaneSpec,
        recovery_enabled: bool,
    ) -> LaneResult:
        repo = lane.payload / "repo"
        output = lane.payload / "reconstructed"
        remove_tree(repo)
        remove_tree(output)
        self._init_repo(lane.root, repo, lane_spec.label)
        sources = sorted(path for path in product.iterdir() if path.is_dir())
        if not sources:
            raise ValueError(f"product has no target directories: {product}")
        backup_times: list[float] = []
        restore_seconds = 0.0
        verify_seconds = 0.0
        snapshots: list[tuple[str, str, Path]] = []
        traces: dict[str, list[str]] = {"capture": [], "reconstruct": [], "corrupt": [], "repair": []}
        for index, source in enumerate(sources, start=1):
            capture_stats = lane.stats / f"capture-{index:03d}.json"
            capture_trace = lane.traces / f"capture-{index:03d}.json"
            elapsed, stats = self._capture(lane.root, repo, source, lane_spec.profile, capture_stats, capture_trace, lane_spec.label)
            backup_times.append(elapsed)
            traces["capture"].append(str(capture_trace))
            destination = output / source.name
            snapshots.append((source.name, _snapshot_id(_stats(capture_stats)), destination))
        reconstructed: list[tuple[str, str, Path]] = []
        for index, (name, snapshot_id, destination) in enumerate(reversed(snapshots), start=1):
            reconstruct_stats = lane.stats / f"reconstruct-{index:03d}.json"
            reconstruct_trace = lane.traces / f"reconstruct-{index:03d}.json"
            elapsed, stats = self._reconstruct(
                repo,
                snapshot_id,
                destination,
                lane_spec.profile,
                reconstruct_stats,
                reconstruct_trace,
                f"{lane_spec.label} {name}",
            )
            restore_seconds += elapsed
            verify_seconds += _verify_seconds(stats)
            traces["reconstruct"].append(str(reconstruct_trace))
            reconstructed.append((name, snapshot_id, destination))
        recovery: dict[str, float | list[str]] = {}
        if recovery_enabled:
            recovery = self._run_corrupt_repair(lane, repo, reconstructed, lane_spec.profile, lane_spec.label)
            traces["corrupt"].extend(recovery["corrupt_traces"])  # type: ignore[arg-type]
            traces["repair"].extend(recovery["repair_traces"])  # type: ignore[arg-type]
        differences = []
        for source in sources:
            differences.extend(compare_directories(source, output / source.name))
        repo_stats = scan_tree(repo)
        return LaneResult(
            lane_spec.label,
            lane_spec.kind,
            {
                "backup": sum(backup_times),
                "restore": restore_seconds,
                "verify": verify_seconds,
                "compare": 0.0 if not differences else 1.0,
                "corrupt": float(recovery.get("corrupt", 0.0)),
                "repair": float(recovery.get("repair", 0.0)),
                "growth": repo_stats.total_bytes,
                "repo_size": repo_stats.total_bytes,
                "repo_files": repo_stats.file_count,
                "backup_min": min(backup_times),
                "backup_max": max(backup_times),
            },
            traces,
            {
                "verify_source": "reconstruct_stats.verify_timings.total_seconds",
                "verify_invocations": len(sources),
                "corrupt_repair_after_reconstruct": recovery_enabled,
                "differences": differences,
            },
        )

    def _run_product_bulk(
        self,
        lane: Any,
        product: Path,
        lane_spec: LaneSpec,
        recovery_enabled: bool,
    ) -> LaneResult:
        repo = lane.payload / "repo"
        output = lane.payload / "reconstructed"
        remove_tree(repo)
        remove_tree(output)
        self._init_repo(lane.root, repo, lane_spec.label)
        capture_stats = lane.stats / "capture.json"
        capture_trace = lane.traces / "capture.json"
        sources = sorted(path for path in product.iterdir() if path.is_dir())
        if not sources:
            raise ValueError(f"product has no target directories: {product}")
        backup, capture = self._capture(
            lane.root,
            repo,
            product,
            lane_spec.profile,
            capture_stats,
            capture_trace,
            lane_spec.label,
            version_name=sources[-1].name,
        )
        reconstruct_stats = lane.stats / "reconstruct.json"
        reconstruct_trace = lane.traces / "reconstruct.json"
        restore, reconstruction = self._reconstruct(repo, _snapshot_id(capture), output, lane_spec.profile, reconstruct_stats, reconstruct_trace, lane_spec.label)
        snapshots = [(product.name, _snapshot_id(capture), output)]
        recovery = self._run_corrupt_repair(lane, repo, snapshots, lane_spec.profile, lane_spec.label) if recovery_enabled else {}
        differences = compare_directories(product, output)
        repo_stats = scan_tree(repo)
        return LaneResult(
            lane_spec.label,
            lane_spec.kind,
            {
                "backup": backup,
                "restore": restore,
                "verify": _verify_seconds(reconstruction),
                "compare": 0.0 if not differences else 1.0,
                "corrupt": float(recovery.get("corrupt", 0.0)),
                "repair": float(recovery.get("repair", 0.0)),
                "growth": repo_stats.total_bytes,
                "repo_size": repo_stats.total_bytes,
                "repo_files": repo_stats.file_count,
            },
            {
                "capture": [str(capture_trace)],
                "reconstruct": [str(reconstruct_trace)],
                "corrupt": recovery.get("corrupt_traces", []),
                "repair": recovery.get("repair_traces", []),
            },
            {
                "verify_source": "reconstruct_stats.verify_timings.total_seconds",
                "verify_invocations": 1,
                "corrupt_repair_after_reconstruct": recovery_enabled,
                "differences": differences,
            },
        )

    def _run_batch(
        self,
        lane: Any,
        product: Path,
        lane_spec: LaneSpec,
        recovery_enabled: bool,
    ) -> LaneResult:
        repo = lane.payload / "repo"
        output = lane.payload / "reconstructed"
        remove_tree(repo)
        remove_tree(output)
        self._init_repo(lane.root, repo, lane_spec.label)
        sources = sorted(path for path in product.iterdir() if path.is_dir())
        if not sources:
            raise ValueError(f"product has no target directories: {product}")
        capture_stats = lane.stats / "capture-many.json"
        capture_trace = lane.traces / "capture-many.json"
        backup, capture = self._capture_many(repo, product, lane_spec.profile, capture_stats, capture_trace, lane_spec.label, lane_spec.label)
        snapshots = capture.get("snapshots", [])
        by_name = {
            Path(item["input_path"]).name: str(item["snapshot_id"])
            for item in snapshots
            if isinstance(item, dict) and item.get("input_path") and item.get("snapshot_id")
        }
        missing = [source.name for source in sources if source.name not in by_name]
        if missing:
            raise ValueError(f"capture-many stats omitted snapshots: {missing}")
        restore = 0.0
        verify = 0.0
        traces: dict[str, list[str]] = {"capture": [str(capture_trace)], "reconstruct": [], "corrupt": [], "repair": []}
        reconstructed: list[tuple[str, str, Path]] = []
        for index, source in enumerate(reversed(sources), start=1):
            destination = output / source.name
            stats = lane.stats / f"reconstruct-{index:03d}.json"
            trace = lane.traces / f"reconstruct-{index:03d}.json"
            elapsed, reconstruction = self._reconstruct(
                repo,
                by_name[source.name],
                destination,
                lane_spec.profile,
                stats,
                trace,
                f"{lane_spec.label} {source.name}",
            )
            restore += elapsed
            verify += _verify_seconds(reconstruction)
            traces["reconstruct"].append(str(trace))
            reconstructed.append((source.name, by_name[source.name], destination))
        recovery: dict[str, float | list[str]] = {}
        if recovery_enabled:
            recovery = self._run_corrupt_repair(lane, repo, reconstructed, lane_spec.profile, lane_spec.label)
            traces["corrupt"].extend(recovery["corrupt_traces"])  # type: ignore[arg-type]
            traces["repair"].extend(recovery["repair_traces"])  # type: ignore[arg-type]
        differences: list[str] = []
        for source in sources:
            differences.extend(compare_directories(source, output / source.name))
        repo_stats = scan_tree(repo)
        return LaneResult(
            lane_spec.label,
            lane_spec.kind,
            {
                "backup": backup,
                "restore": restore,
                "verify": verify,
                "compare": 0.0 if not differences else 1.0,
                "corrupt": float(recovery.get("corrupt", 0.0)),
                "repair": float(recovery.get("repair", 0.0)),
                "growth": repo_stats.total_bytes,
                "repo_size": repo_stats.total_bytes,
                "repo_files": repo_stats.file_count,
            },
            traces,
            {
                "verify_source": "reconstruct_stats.verify_timings.total_seconds",
                "verify_invocations": len(sources),
                "corrupt_repair_after_reconstruct": recovery_enabled,
                "differences": differences,
            },
        )

    def _run_copy(
        self,
        lane: Any,
        product: Path,
        lane_spec: LaneSpec,
        recovery_enabled: bool,
    ) -> LaneResult:
        output = lane.payload / "reconstructed"
        remove_tree(output)
        started = time.perf_counter()
        shutil.copytree(product, output)
        elapsed = time.perf_counter() - started
        corrupt_seconds = 0.0
        repair_seconds = 0.0
        if recovery_enabled:
            plans = [
                build_corruption_plan(
                    output / source.name,
                    lane.stats / f"corruption-{index:03d}.json",
                    index,
                )
                for index, source in enumerate(
                    sorted(path for path in product.iterdir() if path.is_dir()),
                    start=1,
                )
            ]
            corrupt_started = time.perf_counter()
            for source, plan in zip(
                sorted(path for path in product.iterdir() if path.is_dir()),
                plans,
                strict=True,
            ):
                apply_corruption_plan(output / source.name, plan)
            corrupt_seconds = time.perf_counter() - corrupt_started
            remove_tree(output)
            repair_started = time.perf_counter()
            shutil.copytree(product, output)
            repair_seconds = time.perf_counter() - repair_started
        differences = compare_directories(product, output)
        return LaneResult(
            lane_spec.label,
            lane_spec.kind,
            {
                "backup": elapsed,
                "restore": elapsed,
                "verify": 0.0,
                "compare": 0.0 if not differences else 1.0,
                "corrupt": corrupt_seconds,
                "repair": repair_seconds,
            },
            {},
            {
                "verify_source": "not_applicable",
                "verify_invocations": 0,
                "corrupt_repair_after_reconstruct": recovery_enabled,
                "differences": differences,
            },
        )

    def run_lane(
        self,
        product: Path,
        lane_spec: LaneSpec,
        recovery_enabled: bool = False,
    ) -> LaneResult:
        if lane_spec.kind.startswith(("restic-", "kopia-", "borg-", "plakar-")):
            if self.external is None:
                raise RuntimeError("external benchmark tools are not configured")
            return run_external(
                self.external,
                self.run.lane(lane_spec.label),
                product,
                lane_spec,
                recovery_enabled=recovery_enabled,
            )
        lane = self.run.lane(lane_spec.label)
        if lane_spec.kind == "gutch-individual":
            return self._run_individual(lane, product, lane_spec, recovery_enabled)
        if lane_spec.kind == "gutch-product-bulk":
            return self._run_product_bulk(lane, product, lane_spec, recovery_enabled)
        if lane_spec.kind == "gutch-batch":
            return self._run_batch(lane, product, lane_spec, recovery_enabled)
        if lane_spec.kind == "filesystem-copy":
            return self._run_copy(lane, product, lane_spec, recovery_enabled)
        raise ValueError(f"unsupported v2 Gutch lane kind: {lane_spec.kind}")


def run_fixture(
    fixture: FixtureSpec,
    product: Path,
    cli: GutchCli,
    artifacts_root: Path,
    *,
    keep_artifacts: bool,
    repo_root: Path | None = None,
    on_lane_complete: Callable[[LaneResult], None] | None = None,
    on_run_complete: Callable[[list[LaneResult]], None] | None = None,
) -> list[LaneResult]:
    run_root = create_run_dir(artifacts_root, product.name, fixture.name)
    layout = RunLayout(run_root)
    commands = CommandRunner(step_wait_seconds=fixture.step_wait_seconds)
    runner = LocalGutchFixtureRunner(
        cli,
        commands,
        layout,
        ExternalTools(repo_root or Path(__file__).resolve().parents[4], commands),
    )
    results: list[LaneResult] = []
    measured_lanes = [lane for lane in fixture.lanes if not lane.warmup]
    for lane_index, lane_spec in enumerate(measured_lanes):
        commands.begin_lane()
        result = runner.run_lane(product, lane_spec, fixture.corrupt_repair)
        results.append(result)
        if on_lane_complete is not None:
            on_lane_complete(result)
        if lane_index + 1 < len(measured_lanes) and fixture.lane_wait_seconds > 0:
            next_lane = measured_lanes[lane_index + 1]
            print(
                f"\r[wait] next lane starts in {fixture.lane_wait_seconds:g} s: {next_lane.label}",
                flush=True,
            )
            time.sleep(fixture.lane_wait_seconds)
    layout.write_results([result.as_json() for result in results])
    if on_run_complete is not None:
        on_run_complete(results)
    print(f"Wrote benchmark results: {run_root / 'results.json'}")
    if fixture.delete_artifacts and not keep_artifacts:
        layout.remove_payloads()
    combined_trace = run_root / "trace.json"
    combine_labeled_traces(
        [
            (result.label, Path(trace))
            for result in results
            for paths in result.traces.values()
            for trace in paths
        ],
        combined_trace,
    )
    if fixture.open_perfetto:
        trace_events = json.loads(combined_trace.read_text(encoding="utf-8")).get("traceEvents", [])
        if trace_events:
            print(f"Opening Perfetto trace: {combined_trace}")
            open_trace_in_perfetto(combined_trace, print)
        else:
            print("No benchmark Perfetto trace was produced to open.")
    return results
