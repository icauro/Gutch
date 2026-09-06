"""Cold S3 publication and reconstruction workflow for Gutch."""

from dataclasses import dataclass
import json
from pathlib import Path
import time
from collections.abc import Callable
from typing import Any

from gutch_repo.test_benchmarks.common.artifacts import RunLayout, create_run_dir
from gutch_repo.test_benchmarks.common.command import CommandRunner
from gutch_repo.test_benchmarks.common.config import load_scenario
from gutch_repo.test_benchmarks.common.filesystem import compare_directories, remove_tree, scan_tree
from gutch_repo.test_benchmarks.common.traces import combine_labeled_traces, open_trace_in_perfetto
from .commands import GutchCli
from .config import load_fixture
from .models import FixtureSpec, GutchProfile, LaneResult
from .s3 import (
    S3Settings,
    fetch_command,
    make_artifact_url_manifest,
    make_client,
    metadata_urls,
    publish_repository,
)
from .workflow import LocalGutchFixtureRunner, _snapshot_id, _verify_seconds


@dataclass(frozen=True)
class S3CycleOptions:
    prefix_root: str = "hostapp/gutch-benchmark"
    transfer_concurrency: int = 4
    transfer_max_concurrency: int = 16
    upload_concurrency: int = 8


def load_s3_cycle(path: Path) -> tuple[FixtureSpec, S3CycleOptions]:
    fixture = load_fixture(path)
    if fixture.runner != "gutch-s3":
        raise ValueError(f"fixture {fixture.name!r} is not a gutch-s3 fixture")
    values = load_scenario(path).values
    storage = values.get("storage", {})
    transfer = values.get("transfer", {})
    if not isinstance(storage, dict) or not isinstance(transfer, dict):
        raise ValueError(f"S3 fixture storage and transfer must be mappings: {path}")
    options = S3CycleOptions(
        prefix_root=str(storage.get("prefix_root", "hostapp/gutch-benchmark")).strip("/"),
        transfer_concurrency=int(transfer.get("concurrency", 4)),
        transfer_max_concurrency=int(transfer.get("max_concurrency", 16)),
        upload_concurrency=int(storage.get("upload_concurrency", 8)),
    )
    if not options.prefix_root:
        raise ValueError("S3 fixture prefix_root must not be empty")
    if min(
        options.transfer_concurrency,
        options.transfer_max_concurrency,
        options.upload_concurrency,
    ) < 1:
        raise ValueError("S3 fixture concurrency values must be positive")
    if options.transfer_concurrency > options.transfer_max_concurrency:
        raise ValueError("S3 fixture transfer concurrency exceeds max_concurrency")
    return fixture, options


def _source_entries(product: Path, snapshot_limit: int | None = None) -> list[tuple[Path, str]]:
    directories = sorted(
        path for path in product.iterdir() if path.is_dir() and not path.name.startswith(".")
    )
    if snapshot_limit is not None:
        directories = directories[-snapshot_limit:]
    if directories:
        return [(path, path.name) for path in directories]
    return [(product, "1.0.0")]


def _fetch_profile(
    commands: CommandRunner,
    lane: Any,
    cli: GutchCli,
    fetched_repo: Path,
    destination: Path,
    snapshot_id: str,
    profile: GutchProfile,
    urls: dict[str, str],
    manifest: Path,
    options: S3CycleOptions,
    index: int,
    label: str,
) -> tuple[float, dict[str, Any], Path]:
    stats_path = lane.stats / f"fetch-reconstruct-{index:03d}.json"
    trace_path = lane.traces / f"fetch-reconstruct-{index:03d}.json"
    result = commands.run(
        [
            *fetch_command(
                cli,
                fetched_repo,
                snapshot_id,
                urls,
                manifest,
                profile,
                transfer_concurrency=options.transfer_concurrency,
                transfer_max_concurrency=options.transfer_max_concurrency,
                destination=destination,
                stats_path=stats_path,
                trace_path=trace_path,
            ),
        ],
        description=f"{label} fetch-reconstruct",
    )
    return result.elapsed_seconds, json.loads(stats_path.read_text(encoding="utf-8")), trace_path


def run_s3_cycle(
    fixture: FixtureSpec,
    product: Path,
    cli: GutchCli,
    settings: S3Settings,
    artifacts_root: Path,
    *,
    options: S3CycleOptions | None = None,
    keep_artifacts: bool = False,
    on_lane_complete: Callable[[LaneResult], None] | None = None,
    on_run_complete: Callable[[list[LaneResult]], None] | None = None,
    on_step: Callable[[str], None] | None = None,
) -> list[LaneResult]:
    if fixture.runner != "gutch-s3":
        raise ValueError(f"fixture {fixture.name!r} is not a gutch-s3 fixture")
    measured = [lane for lane in fixture.lanes if not lane.warmup]
    if any(lane.kind != "gutch-individual" for lane in measured):
        raise ValueError("S3 cycle lanes must all be gutch-individual")
    options = options or S3CycleOptions()
    run_root = create_run_dir(artifacts_root, product.name, fixture.name)
    layout = RunLayout(run_root)
    commands = CommandRunner(step_wait_seconds=fixture.step_wait_seconds, on_step=on_step)
    local = LocalGutchFixtureRunner(
        cli,
        commands,
        layout,
        external=None,
    )
    client = make_client(settings)
    entries = _source_entries(product, fixture.snapshot_limit)
    results: list[LaneResult] = []
    for lane_index, lane_spec in enumerate(measured):
        commands.begin_lane()
        if on_step is not None:
            on_step(f"{lane_spec.label} lane started")
        lane = layout.lane(lane_spec.label)
        repo = lane.payload / "published-repo"
        fetched_repo_root = lane.payload / "fetched-repo"
        output = lane.payload / "reconstructed"
        remove_tree(repo)
        remove_tree(fetched_repo_root)
        remove_tree(output)
        local._init_repo(repo, lane_spec.label)
        captures: list[tuple[str, str]] = []
        backup_seconds = 0.0
        traces: dict[str, list[str]] = {
            "capture": [],
            "fetch_reconstruct": [],
            "corrupt": [],
            "repair": [],
        }
        for index, (source, version_name) in enumerate(entries, start=1):
            stats_path = lane.stats / f"capture-{index:03d}.json"
            trace_path = lane.traces / f"capture-{index:03d}.json"
            elapsed, stats = local._capture(
                lane.root,
                repo,
                source,
                lane_spec.profile,
                stats_path,
                trace_path,
                lane_spec.label,
                version_name=version_name,
            )
            backup_seconds += elapsed
            captures.append((source.name, _snapshot_id(stats)))
            traces["capture"].append(str(trace_path))
        prefix = f"{options.prefix_root}/{run_root.name}/{lane_spec.label}"
        if on_step is not None:
            on_step(f"{lane_spec.label} publishing repository to S3")
        upload_started = time.perf_counter()
        upload = publish_repository(
            client,
            settings,
            repo,
            prefix,
            workers=options.upload_concurrency,
        )
        upload_seconds = time.perf_counter() - upload_started
        manifest = lane.root / "artifact-urls.json"
        if on_step is not None:
            on_step(f"{lane_spec.label} generating artifact URL manifest")
        make_artifact_url_manifest(client, settings, repo, prefix, manifest)
        fetch_seconds = 0.0
        verify_seconds = 0.0
        differences: list[str] = []
        fetch_records: list[tuple[str, str, Path, Path]] = []
        for index, (name, snapshot_id) in enumerate(reversed(captures), start=1):
            if on_step is not None:
                on_step(f"{lane_spec.label} fetching and reconstructing {name}")
            urls = metadata_urls(client, settings, repo, prefix, snapshot_id)
            destination = output / name if len(entries) > 1 else output
            fetched_repo = fetched_repo_root / f"{index:03d}"
            elapsed, stats, trace_path = _fetch_profile(
                commands,
                lane,
                cli,
                fetched_repo,
                destination,
                snapshot_id,
                lane_spec.profile,
                urls,
                manifest,
                options,
                index,
                name,
            )
            fetch_seconds += elapsed
            verify_seconds += _verify_seconds(stats)
            traces["fetch_reconstruct"].append(str(trace_path))
            fetch_records.append((name, snapshot_id, fetched_repo, destination))
            source = next(source for source, source_name in entries if source_name == name)
            differences.extend(compare_directories(source, destination))
        corrupt_seconds = 0.0
        repair_seconds = 0.0
        if fixture.corrupt_repair:
            for name, snapshot_id, fetched_repo, destination in fetch_records:
                recovery = local._run_corrupt_repair(
                    lane,
                    fetched_repo,
                    [(name, snapshot_id, destination)],
                    lane_spec.profile,
                    lane_spec.label,
                )
                corrupt_seconds += float(recovery["corrupt"])
                repair_seconds += float(recovery["repair"])
                traces["corrupt"].extend(recovery["corrupt_traces"])  # type: ignore[arg-type]
                traces["repair"].extend(recovery["repair_traces"])  # type: ignore[arg-type]
            differences = []
            for source, source_name in entries:
                destination = output / source_name if len(entries) > 1 else output
                differences.extend(compare_directories(source, destination))
        repo_stats = scan_tree(repo)
        result = LaneResult(
            lane_spec.label,
            lane_spec.kind,
            {
                "backup": backup_seconds,
                "upload": upload_seconds,
                "fetch_reconstruct": fetch_seconds,
                "verify": verify_seconds,
                "corrupt": corrupt_seconds,
                "repair": repair_seconds,
                "compare": 0.0 if not differences else 1.0,
                "repo_size": repo_stats.total_bytes,
                "repo_files": repo_stats.file_count,
                "uploaded_bytes": upload["bytes"],
                "uploaded_objects": upload["object_count"],
            },
            traces,
            {
                "verify_source": "fetch_reconstruct_stats.verify_timings.total_seconds",
                "verify_invocations": len(entries),
                "corrupt_repair_after_reconstruct": fixture.corrupt_repair,
                "s3_prefix": prefix,
                "differences": differences,
            },
        )
        results.append(result)
        if on_lane_complete is not None:
            on_lane_complete(result)
        if lane_index + 1 < len(measured) and fixture.lane_wait_seconds > 0:
            next_lane = measured[lane_index + 1]
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
            for traces in result.traces.values()
            for trace in traces
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
