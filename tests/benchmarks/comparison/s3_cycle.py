"""Cold S3 publication and reconstruction workflow for Gutch."""

from dataclasses import dataclass
import json
from pathlib import Path
import time
from collections.abc import Callable
from typing import Any

from tests.benchmarks.support.artifacts import RunLayout, create_run_dir
from tests.benchmarks.support.command import CommandRunner
from tests.benchmarks.scheduling.config import load_scenario
from tests.benchmarks.support.filesystem import remove_tree, scan_tree
from tests.benchmarks.support.commands import GutchCli
from tests.benchmarks.comparison.config import load_fixture
from tests.benchmarks.scheduling.dataset import (
    DatasetSelection,
    select_sources,
)
from tests.benchmarks.scheduling.runner import execute_cases
from tests.benchmarks.comparison.models import (
    FixtureSpec,
    GutchProfile,
    LaneResult,
)
from tests.benchmarks.support.s3 import (
    S3Settings,
    fetch_command,
    make_artifact_url_manifest,
    make_client,
    metadata_urls,
    publish_repository,
)
from tests.benchmarks.support.gutch import (
    GutchOperations,
    snapshot_id as _snapshot_id,
    verification_seconds as _verify_seconds,
)


@dataclass(frozen=True)
class S3CycleOptions:
    prefix_root: str = "gutch/benchmark"
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
        prefix_root=str(storage.get("prefix_root", "gutch/benchmark")).strip("/"),
        transfer_concurrency=int(transfer.get("concurrency", 4)),
        transfer_max_concurrency=int(transfer.get("max_concurrency", 16)),
        upload_concurrency=int(storage.get("upload_concurrency", 8)),
    )
    if not options.prefix_root:
        raise ValueError("S3 fixture prefix_root must not be empty")
    if (
        min(
            options.transfer_concurrency,
            options.transfer_max_concurrency,
            options.upload_concurrency,
        )
        < 1
    ):
        raise ValueError("S3 fixture concurrency values must be positive")
    if options.transfer_concurrency > options.transfer_max_concurrency:
        raise ValueError("S3 fixture transfer concurrency exceeds max_concurrency")
    return fixture, options


def _source_entries(
    product: Path, snapshot_limit: int | None = None
) -> list[tuple[Path, str]]:
    directories = select_sources(product, snapshot_limit)
    return (
        [(path, path.name) for path in directories]
        if directories
        else [(product, product.name)]
    )


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
    trace_path = lane.traces / f"fetch-reconstruct-{index:03d}" / "trace.json"
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
    return (
        result.elapsed_seconds,
        json.loads(stats_path.read_text(encoding="utf-8")),
        trace_path,
    )


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
    commands = CommandRunner(
        step_wait_seconds=fixture.step_wait_seconds,
        on_step=on_step,
        memory_profile=fixture.profile_mode == "detailed",
    )
    cli = GutchCli(cli.executable, profile_mode=fixture.profile_mode)
    local = GutchOperations(cli, commands)
    client = make_client(settings)
    entries = _source_entries(product, fixture.snapshot_limit)
    selection = DatasetSelection.resolve(
        product,
        fixture.snapshot_limit,
        whole_product=len(entries) == 1 and entries[0][0] == product,
    )

    def execute_lane(lane_spec):
        lane = layout.lane(lane_spec.label)
        repo = lane.payload / "published-repo"
        fetched_repo_root = lane.payload / "fetched-repo"
        output = lane.payload / "installation"
        remove_tree(repo)
        remove_tree(fetched_repo_root)
        remove_tree(output)
        local.init_repo(repo, lane_spec.label)
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
            trace_path = lane.traces / f"capture-{index:03d}" / "trace.json"
            elapsed, stats = local.capture(
                repo,
                source,
                lane_spec.profile,
                stats_path,
                trace_path,
                lane_spec.label,
                version_name=version_name if source != product else "1.0.0",
            )
            backup_seconds += elapsed
            captures.append((source.name, _snapshot_id(stats)))
            traces["capture"].append(str(trace_path))
        from uuid import uuid4
        from tests.benchmarks.support.s3 import delete_published_prefix

        prefix = (
            f"{options.prefix_root}/{run_root.name}/{lane_spec.label}/{uuid4().hex}"
        )
        layout.cleanup_actions.append(
            lambda prefix=prefix: delete_published_prefix(client, settings, prefix)
        )
        layout.write_json(
            lane.root / "publication.json",
            {"bucket": settings.bucket, "prefix": prefix},
        )
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
            measured_verify_seconds = _verify_seconds(stats)
            verify_seconds = (
                verify_seconds + measured_verify_seconds
                if verify_seconds is not None and measured_verify_seconds is not None
                else None
            )
            traces["fetch_reconstruct"].append(str(trace_path))
            fetch_records.append((name, snapshot_id, fetched_repo, destination))
            source = next(
                source for source, source_name in entries if source_name == name
            )
            selection.validate(
                source, destination, lane.stats / f"validate-restored-{index:03d}.json"
            )
        corrupt_seconds = 0.0
        repair_seconds = 0.0
        if fixture.corrupt_repair:
            for name, snapshot_id, fetched_repo, destination in fetch_records:
                recovery = local.corrupt_and_repair(
                    lane,
                    fetched_repo,
                    [(name, snapshot_id, destination)],
                    lane_spec.profile,
                    f"{lane_spec.label}-{name}",
                )
                corrupt_seconds += float(recovery["corrupt"])
                repair_seconds += float(recovery["repair"])
                traces["corrupt"].extend(recovery["corrupt_traces"])  # type: ignore[arg-type]
                traces["repair"].extend(recovery["repair_traces"])  # type: ignore[arg-type]
            for source, source_name in entries:
                destination = output / source_name if len(entries) > 1 else output
                selection.validate(
                    source,
                    destination,
                    lane.stats / f"validate-final-{source.name}.json",
                )
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
                "compare": 0.0,
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
                "differences": [],
            },
        )
        result.metadata.update(
            selection.metadata(),
            configuration=__import__("dataclasses").asdict(lane_spec),
        )
        result.metrics.update(
            input_bytes=selection.input_bytes, input_files=selection.input_files
        )
        return result

    return execute_cases(
        fixture,
        layout,
        commands,
        execute_lane,
        executable=cli.executable,
        keep_artifacts=keep_artifacts,
        on_lane_complete=on_lane_complete,
        on_run_complete=on_run_complete,
    )
