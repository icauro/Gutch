"""S3 reconstruction workflow with verification owned by reconstruct commands."""

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

from gutch_repo.test_benchmarks.common.artifacts import RunLayout, create_run_dir
from gutch_repo.test_benchmarks.common.command import CommandRunner
from gutch_repo.test_benchmarks.common.filesystem import compare_directories, remove_tree, scan_tree
from gutch_repo.test_benchmarks.common.traces import combine_labeled_traces, open_trace_in_perfetto
from .commands import GutchCli
from .models import GutchProfile
from .s3 import (
    S3Settings,
    fetch_command,
    make_artifact_url_manifest,
    make_client,
    metadata_urls,
    publish_repository,
)
from .workflow import LocalGutchFixtureRunner, _snapshot_id, _verify_seconds


def run_reconstruction_storage(
    product: Path,
    cli: GutchCli,
    settings: S3Settings,
    artifacts_root: Path,
    *,
    profile: GutchProfile | None = None,
    transfer_concurrency: int = 4,
    upload_concurrency: int = 8,
    recovery_enabled: bool = False,
    open_perfetto: bool = False,
    step_wait_seconds: float = 4.0,
    keep_artifacts: bool = False,
    run_label: str | None = None,
    include_fetch_reconstruct: bool = True,
    on_run_complete: Callable[[dict[str, Any]], None] | None = None,
) -> dict[str, Any]:
    profile = profile or GutchProfile("reconstruction-storage")
    run_root = create_run_dir(artifacts_root, run_label or product.name, "reconstruction_storage")
    layout = RunLayout(run_root)
    lane = layout.lane("s3-reconstruction")
    commands = CommandRunner(step_wait_seconds=step_wait_seconds)
    local = LocalGutchFixtureRunner(cli, commands, layout)
    repo = lane.payload / "published-repo"
    local_destination = lane.payload / "local-reconstructed"
    fetched_repo = lane.payload / "fetched-repo"
    fetched_destination = lane.payload / "fetched-reconstructed"
    full_repo = lane.payload / "fetch-reconstruct-repo"
    full_destination = lane.payload / "fetch-reconstructed"
    commands.run(
        cli.command("repo", "init", "--path", repo),
        description="reconstruction-storage repository init",
    )
    capture_stats = lane.stats / "capture.json"
    capture_trace = lane.traces / "capture.json"
    capture_elapsed, capture = local._capture(
        lane.root,
        repo,
        product,
        profile,
        capture_stats,
        capture_trace,
        "reconstruction-storage",
        version_name="1.0.0",
    )
    snapshot_id = _snapshot_id(capture)
    local_elapsed, local_stats = local._reconstruct(
        repo,
        snapshot_id,
        local_destination,
        profile,
        lane.stats / "local-reconstruct.json",
        lane.traces / "local-reconstruct.json",
        "local filesystem",
    )

    client = make_client(settings)
    prefix = f"hostapp/gutch-reconstruction-benchmark/{run_root.name}"
    upload = publish_repository(client, settings, repo, prefix, workers=upload_concurrency)
    manifest = lane.root / "artifact-urls.json"
    make_artifact_url_manifest(client, settings, repo, prefix, manifest)
    urls = metadata_urls(client, settings, repo, prefix, snapshot_id)

    fetch_elapsed_result = commands.run(
        fetch_command(
            cli,
            fetched_repo,
            snapshot_id,
            urls,
            manifest,
            profile,
            transfer_concurrency=transfer_concurrency,
            trace_path=lane.traces / "fetch.json",
        ),
        description="S3 fetch",
    )
    fetched_elapsed, fetched_stats = local._reconstruct(
        fetched_repo,
        snapshot_id,
        fetched_destination,
        profile,
        lane.stats / "fetched-reconstruct.json",
        lane.traces / "fetched-reconstruct.json",
        "S3 fetched repository",
    )
    full_result = None
    full_stats: dict[str, Any] | None = None
    if include_fetch_reconstruct:
        full_result = commands.run(
            fetch_command(
                cli,
                full_repo,
                snapshot_id,
                urls,
                manifest,
                profile,
                transfer_concurrency=transfer_concurrency,
                destination=full_destination,
                stats_path=lane.stats / "fetch-reconstruct.json",
                trace_path=lane.traces / "fetch-reconstruct.json",
            ),
            description="S3 fetch-reconstruct",
        )
        full_stats = json.loads((lane.stats / "fetch-reconstruct.json").read_text(encoding="utf-8"))
    corrupt_seconds = 0.0
    repair_seconds = 0.0
    recovery_traces: dict[str, list[str]] = {"corrupt": [], "repair": []}
    if recovery_enabled:
        recovery_targets = [
            ("local", repo, local_destination),
            ("fetched", fetched_repo, fetched_destination),
        ]
        if include_fetch_reconstruct:
            recovery_targets.append(("fetch-reconstruct", full_repo, full_destination))
        for label, fetched_path, destination in recovery_targets:
            recovery = local._run_corrupt_repair(
                lane,
                fetched_path,
                [(label, snapshot_id, destination)],
                profile,
                label,
            )
            corrupt_seconds += float(recovery["corrupt"])
            repair_seconds += float(recovery["repair"])
            recovery_traces["corrupt"].extend(recovery["corrupt_traces"])  # type: ignore[arg-type]
            recovery_traces["repair"].extend(recovery["repair_traces"])  # type: ignore[arg-type]
    traces = {
        "capture": [str(capture_trace)],
        "reconstruct": [
            str(lane.traces / "local-reconstruct.json"),
            str(lane.traces / "fetched-reconstruct.json"),
        ],
        "fetch": [str(lane.traces / "fetch.json")],
        **recovery_traces,
    }
    if include_fetch_reconstruct:
        traces["fetch_reconstruct"] = [str(lane.traces / "fetch-reconstruct.json")]
    differences = {
        "local": compare_directories(product, local_destination),
        "fetched": compare_directories(product, fetched_destination),
    }
    if include_fetch_reconstruct:
        differences["fetch_reconstruct"] = compare_directories(product, full_destination)
    timings: dict[str, float] = {
        "capture": capture_elapsed,
        "local_reconstruct": local_elapsed,
        "fetch": fetch_elapsed_result.elapsed_seconds,
        "fetched_reconstruct": fetched_elapsed,
        "corrupt": corrupt_seconds,
        "repair": repair_seconds,
    }
    if full_result is not None:
        timings["fetch_reconstruct"] = full_result.elapsed_seconds
    verification: dict[str, Any] = {
        "invocations": 2 + int(include_fetch_reconstruct),
        "local_seconds": _verify_seconds(local_stats),
        "fetched_seconds": _verify_seconds(fetched_stats),
        "source": "reconstruct stats; no post-reconstruct verify command",
        "corrupt_repair_after_reconstruct": recovery_enabled,
    }
    if full_stats is not None:
        verification["fetch_reconstruct_seconds"] = _verify_seconds(full_stats)
    result = {
        "run_directory": str(run_root),
        "snapshot_id": snapshot_id,
        "upload": upload,
        "timings": timings,
        "verification": verification,
        "traces": traces,
        "repository": scan_tree(repo).__dict__,
        "differences": differences,
    }
    (run_root / "results.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if on_run_complete is not None:
        on_run_complete(result)
    combined_trace = run_root / "trace.json"
    combine_labeled_traces(
        [("s3-reconstruction", Path(trace)) for paths in traces.values() for trace in paths],
        combined_trace,
    )
    print(f"Wrote benchmark results: {run_root / 'results.json'}")
    if not keep_artifacts:
        layout.remove_payloads()
    if open_perfetto:
        trace_events = json.loads(combined_trace.read_text(encoding="utf-8")).get("traceEvents", [])
        if trace_events:
            print(f"Opening Perfetto trace: {combined_trace}")
            open_trace_in_perfetto(combined_trace, print)
        else:
            print("No benchmark Perfetto trace was produced to open.")
    return result
