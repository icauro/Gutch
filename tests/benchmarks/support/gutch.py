"""Gutch operations; no fixture scheduling or run cleanup policy."""
from dataclasses import dataclass
import json
import math
import re
from pathlib import Path
from typing import Any
from tests.benchmarks.support.commands import GutchCli
from tests.benchmarks.support.command import CommandRunner
from tests.benchmarks.support.models import GutchProfile
from tests.benchmarks.support.artifacts import LaneLayout, safe_component
from tests.benchmarks.support.recovery import build_corruption_plan

def snapshot_version(name: str) -> str:
    if re.fullmatch(r"v[1-9][0-9]*", name):
        return f"{name[1:]}.0.0"
    match = re.search(r"(?<!\d)(\d+\.\d+\.\d+(?:\.\d+)?)(?!\d)", name)
    if match is None:
        raise ValueError(f"target {name!r} must contain a semantic version")
    return match.group(1)


def read_stats(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def snapshot_id(stats: dict[str, Any]) -> str:
    value = stats.get("snapshot_id")
    if not isinstance(value, str) or not value:
        raise ValueError("capture stats did not contain snapshot_id")
    return value


def verification_seconds(stats: dict[str, Any]) -> float | None:
    timings = stats.get("verify_timings")
    if not isinstance(timings, dict):
        raise ValueError("missing verification timings in Gutch result")
    value = timings.get("total_seconds")
    if value is None and stats.get("profiling_mode") in {"off", "summary"}:
        return None
    if not isinstance(value, (int, float)) or isinstance(value, bool) or not math.isfinite(value) or value < 0:
        raise ValueError("invalid verification duration in Gutch result")
    return float(value)


@dataclass
class GutchOperations:
    cli: GutchCli
    commands: CommandRunner

    def init_repo(self, repo: Path, label: str) -> None:
        self.commands.run(
            self.cli.command("repo", "init", "--repo", repo),
            description=f"{label} repository init",
        )

    def capture(
        self,
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
                "--source",
                source,
                "--repo",
                repo,
                "--yes",
                "--tag",
                f"version={snapshot_version(version_name or source.name)}",
                "--tag",
                f"benchmark={tag.replace('-', '_')}",
                *GutchCli.profile_args(profile),
                "--json", "--output",
                stats_path,
                "--profile-dir",
                trace_path.parent,
            ),
            description=f"{tag} capture {source.name}",
        )
        return result.elapsed_seconds, read_stats(stats_path)


    def reconstruct(
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
                "installation",
                "reconstruct",
                "--repo",
                repo,
                "--snapshot",
                snapshot_id,
                "--destination",
                destination,
                *GutchCli.runtime_args(profile),
                "--json", "--output",
                stats_path,
                "--profile-dir",
                trace_path.parent,
            ),
            description=f"reconstruct {label}",
        )
        return result.elapsed_seconds, read_stats(stats_path)

    def repair(self, lane, repo, snapshots, profile, label, operation):
        total = 0.0
        for index, (name, snapshot, destination) in enumerate(snapshots, 1):
            result = self.commands.run(self.cli.command(
                "installation", "repair", "--repo", repo, "--snapshot", snapshot,
                "--destination", destination, "--remove-extra-files", "--yes",
                *GutchCli.runtime_args(profile), "--json", "--output", lane.stats / f"{operation}-{index}.json",
                "--profile-dir", lane.traces / f"{operation}-{index}"), description=f"{label} {operation} {name}")
            total += result.elapsed_seconds
        return total

    def corrupt_and_repair(
        self,
        lane: LaneLayout,
        repo: Path,
        snapshots: list[tuple[str, str, Path]],
        profile: GutchProfile,
        label: str,
    ) -> dict[str, float | list[str]]:
        """Damages each installation with a seeded plan, then repairs it."""
        safe = safe_component(label)
        corrupt_seconds = 0.0
        repair_seconds = 0.0
        corrupt_traces: list[str] = []
        repair_traces: list[str] = []
        for index, (name, snapshot, destination) in enumerate(snapshots, 1):
            plan = build_corruption_plan(
                destination, lane.stats / f"corruption-{safe}-{index:03d}.json", index
            )
            trace = lane.traces / f"corrupt-{safe}-{index:03d}" / "trace.json"
            result = self.commands.run(
                self.cli.command(
                    "debug", "corrupt-installation", "--destination", destination,
                    "--plan", plan, "--json", "--output",
                    lane.stats / f"corrupt-{safe}-{index:03d}.json", "--yes",
                    "--profile-dir", trace.parent,
                ),
                description=f"{label} corrupt {name}",
            )
            corrupt_seconds += result.elapsed_seconds
            corrupt_traces.append(str(trace))
            trace = lane.traces / f"repair-{safe}-{index:03d}" / "trace.json"
            result = self.commands.run(
                self.cli.command(
                    "installation", "repair", "--repo", repo, "--snapshot", snapshot,
                    "--destination", destination, "--remove-extra-files", "--yes",
                    *GutchCli.runtime_args(profile), "--json", "--output",
                    lane.stats / f"repair-{safe}-{index:03d}.json",
                    "--profile-dir", trace.parent,
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
