"""Gutch-only capture modes and diagnostic scheduling."""
from collections.abc import Callable
from pathlib import Path
from typing import Any

from comparison_benchmark_suite.support.artifacts import LaneLayout, RunLayout, create_run_dir
from comparison_benchmark_suite.support.command import CommandRunner
from comparison_benchmark_suite.support.commands import GutchCli
from comparison_benchmark_suite.support.filesystem import remove_tree, scan_tree
from comparison_benchmark_suite.support.gutch import read_stats, snapshot_id as _snapshot_id, verification_seconds as _verify_seconds
from comparison_benchmark_suite.scheduling.dataset import DatasetSelection, select_sources as _selected_product_sources
from comparison_benchmark_suite.scheduling.runner import execute_cases
from comparison_benchmark_suite.tools.models import FixtureSpec, LaneResult, LaneSpec
from comparison_benchmark_suite.tools.workflow import LocalFixtureRunner


class GutchDiagnosticRunner(LocalFixtureRunner):
    def run_lane(self, product, lane_spec, recovery_enabled=False, snapshot_limit=None):
        dataset = DatasetSelection.resolve(product, whole_product=True) if lane_spec.workload == "product-bulk" else None
        return super().run_lane(product, lane_spec, recovery_enabled, snapshot_limit, dataset=dataset)

    def _execute_lane(self, dataset, lane_spec, recovery_enabled=False, snapshot_limit=None):
        lane = self.run.lane(lane_spec.label)
        if lane_spec.workload == "batch":
            return self._run_batch(lane, dataset, lane_spec, recovery_enabled)
        if lane_spec.workload == "product-bulk":
            return self._run_product_bulk(lane, dataset, lane_spec, recovery_enabled)
        return super()._execute_lane(dataset, lane_spec, recovery_enabled, snapshot_limit)

    def _capture_many(
        self,
        repo: Path,
        source_root: Path,
        profile,
        stats_path: Path,
        trace_path: Path,
        tag: str,
        label: str,
    ) -> tuple[float, dict[str, Any]]:
        result = self.commands.run(
            self.cli.command(
                "snapshot",
                "capture",
                "--discover-from",
                source_root,
                "--repo",
                repo,
                "--yes",
                "--tag",
                f"benchmark={tag.replace('-', '_')}",
                *GutchCli.profile_args(profile),
                "--json", "--output",
                stats_path,
                "--profile-dir",
                trace_path.parent,
            ),
            description=f"capture-many {label}",
        )
        return result.elapsed_seconds, read_stats(stats_path)


    def _run_product_bulk(
        self,
        lane: LaneLayout,
        dataset: DatasetSelection,
        lane_spec: LaneSpec,
        recovery_enabled: bool,
    ) -> LaneResult:
        product = dataset.product
        repo = lane.payload / "repo"
        output = lane.payload / "installation"
        remove_tree(repo)
        remove_tree(output)
        self.gutch.init_repo(repo, lane_spec.label)
        capture_stats = lane.stats / "capture.json"
        capture_trace = lane.traces / "capture" / "trace.json"
        sources = _selected_product_sources(product)
        if not sources:
            raise ValueError(f"product has no target directories: {product}")
        backup, capture = self.gutch.capture(
            repo,
            product,
            lane_spec.profile,
            capture_stats,
            capture_trace,
            lane_spec.label,
            version_name=sources[-1].name,
        )
        repo_stats = scan_tree(repo)
        reconstruct_stats = lane.stats / "reconstruct.json"
        reconstruct_trace = lane.traces / "reconstruct" / "trace.json"
        restore, reconstruction = self.gutch.reconstruct(repo, _snapshot_id(capture), output, lane_spec.profile, reconstruct_stats, reconstruct_trace, lane_spec.label)
        dataset.validate(product, output, lane.stats / "validate-restored.json")
        snapshot_metrics = dataset.snapshot_metrics()
        snapshot_metrics[product.name].update(backup=backup, restore=restore)
        snapshots = [(product.name, _snapshot_id(capture), output)]
        recovery = self.gutch.corrupt_and_repair(lane, repo, snapshots, lane_spec.profile, lane_spec.label) if recovery_enabled else {}
        dataset.validate(product, output, lane.stats / "validate-final.json")
        return LaneResult(
            lane_spec.label,
            lane_spec.kind,
            {
                "backup": backup,
                "restore": restore,
                "verify": _verify_seconds(reconstruction),
                "compare": 0.0,
                "corrupt": float(recovery.get("corrupt", 0.0)),
                "repair": float(recovery.get("repair", 0.0)),
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
                "snapshot_metrics": snapshot_metrics,
                "verify_source": "reconstruct_stats.verify_timings.total_seconds",
                "verify_invocations": 1,
                "corrupt_repair_after_reconstruct": recovery_enabled,
                "differences": [],
            },
        )


    def _run_batch(
        self,
        lane: LaneLayout,
        dataset: DatasetSelection,
        lane_spec: LaneSpec,
        recovery_enabled: bool,
    ) -> LaneResult:
        product = dataset.product
        repo = lane.payload / "repo"
        output = lane.payload / "installation"
        remove_tree(repo)
        remove_tree(output)
        self.gutch.init_repo(repo, lane_spec.label)
        sources = list(dataset.sources)
        if not sources:
            raise ValueError(f"product has no target directories: {product}")
        capture_stats = lane.stats / "capture-many.json"
        capture_trace = lane.traces / "capture-many" / "trace.json"
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
        snapshots = [(source.name, by_name[source.name], output / source.name) for source in sources]
        return self._restore_gutch_snapshots(
            lane, dataset, lane_spec, snapshots, backup, [str(capture_trace)],
            dataset.snapshot_metrics(), recovery_enabled,
        )



def run_fixture(
    fixture: FixtureSpec,
    product: Path,
    cli: GutchCli,
    artifacts_root: Path,
    *,
    keep_artifacts: bool,
    on_lane_complete: Callable[[LaneResult], None] | None = None,
    on_run_complete: Callable[[list[LaneResult]], None] | None = None,
    on_step: Callable[[str], None] | None = None,
) -> list[LaneResult]:
    """Run Gutch's internal diagnostics using the shared measurement runtime."""
    unsupported_tools = sorted({lane.tool for lane in fixture.lanes} - {"gutch", "filesystem"})
    if unsupported_tools:
        raise ValueError("External-tool comparisons belong in the standalone suite: " + ", ".join(unsupported_tools))
    if fixture.snapshot_limit is not None:
        supported_kinds = {
            "gutch-individual",
            "filesystem-copy",
        }
        unsupported = sorted(
            {lane.kind for lane in fixture.lanes if lane.kind not in supported_kinds}
        )
        if unsupported:
            raise ValueError(
                "snapshot_limit is only supported for individual snapshot lanes: "
                + ", ".join(unsupported)
            )
    run_root = create_run_dir(artifacts_root, product.name, fixture.name)
    layout = RunLayout(run_root)
    commands = CommandRunner(step_wait_seconds=fixture.step_wait_seconds, on_step=on_step, memory_profile=fixture.profile_mode == "detailed")
    cli = GutchCli(cli.executable, profile_mode=fixture.profile_mode)
    runner = GutchDiagnosticRunner(
        cli,
        commands,
        layout,
    )
    return execute_cases(fixture, layout, commands,
        lambda case: runner.run_lane(product, case, fixture.corrupt_repair, snapshot_limit=fixture.snapshot_limit),
        executable=cli.executable, keep_artifacts=keep_artifacts,
        on_lane_complete=on_lane_complete, on_run_complete=on_run_complete)
