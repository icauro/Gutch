"""Adapters for the local Restic, Kopia, Borg, and Plakar comparison lanes."""

from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import tempfile
import time

from gutch_repo.tests.benchmarks.common.command import CommandRunner
from gutch_repo.tests.benchmarks.common.filesystem import compare_directories, remove_tree, scan_tree
from gutch_repo.tests.benchmarks.common.recovery import apply_corruption_plan, build_corruption_plan
from .models import LaneResult, LaneSpec


@dataclass
class ExternalTools:
    repo_root: Path
    commands: CommandRunner
    restic: Path | None = None
    kopia: Path | None = None
    borg: Path | None = None
    plakar: Path | None = None

    def executable(self, kind: str) -> Path:
        explicit = {
            "restic": self.restic,
            "kopia": self.kopia,
            "borg": self.borg,
            "plakar": self.plakar,
        }.get(kind)
        if explicit and explicit.exists():
            return explicit
        candidate = self.repo_root / "gutch_repo" / "tests" / "benchmarks" / "gutch_fixture_runner" / "bin"
        if os.name == "nt":
            if kind == "plakar":
                candidate = candidate / "plakar" / "plakar.exe"
            else:
                candidate /= f"{kind}.exe"
        else:
            candidate = candidate / "linux-x64" / kind
        if candidate.exists():
            return candidate
        raise FileNotFoundError(f"{kind} executable is not configured: {candidate}")


def _restic_id(output: str) -> str:
    for line in output.splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict) and value.get("message_type") == "summary" and value.get("snapshot_id"):
            return str(value["snapshot_id"])
    raise RuntimeError("restic backup did not return a snapshot id")


def _compare(expected: list[Path], actual_root: Path) -> list[str]:
    differences: list[str] = []
    for source in expected:
        differences.extend(compare_directories(source, actual_root / source.name))
    return differences


def _plakar_cache_directory(lane_root: Path) -> Path:
    lane_id = hashlib.sha256(str(lane_root.resolve()).encode("utf-8")).hexdigest()[:12]
    return Path(tempfile.gettempdir()) / f"gutch-p-{lane_id}"


def _plakar_command(
    executable: Path,
    repo: Path,
    cache_directory: Path,
    *arguments: str | Path,
) -> list[str | Path]:
    return [
        executable,
        "-quiet",
        "-cachedir",
        cache_directory,
        "-stdio",
        "at",
        repo,
        *arguments,
    ]


def run_external(
    tools: ExternalTools,
    lane: object,
    product: Path,
    lane_spec: LaneSpec,
    *,
    recovery_enabled: bool = False,
    snapshot_limit: int | None = None,
) -> LaneResult:
    kind = lane_spec.kind.split("-", 1)[0]
    executable = tools.executable(kind)
    lane_root = lane.root
    plakar_cache_directory = _plakar_cache_directory(lane_root)
    repo = lane.payload / "repo"
    output = lane.payload / "reconstructed"
    remove_tree(repo)
    remove_tree(output)
    repo.mkdir(parents=True, exist_ok=True)
    output.mkdir(parents=True, exist_ok=True)
    sources = sorted(path for path in product.iterdir() if path.is_dir())
    if not sources:
        raise ValueError(f"product has no target directories: {product}")
    if snapshot_limit is not None:
        sources = sources[-snapshot_limit:]
    env = {}
    if kind == "restic":
        env = {
            "RESTIC_PASSWORD": "bench",
            "RESTIC_CACHE_DIR": str(lane_root / "restic-cache"),
        }
        tools.commands.run(
            [executable, "init", *lane_spec.arguments, "--repo", repo],
            description=f"{lane_spec.label} init",
            env=env,
        )
    elif kind == "kopia":
        env = {
            "KOPIA_CONFIG_PATH": str(lane_root / "kopia.config"),
            "KOPIA_PASSWORD": "bench",
            "KOPIA_CACHE_DIRECTORY": str(lane_root / "kopia-cache"),
            "KOPIA_LOG_DIR": str(lane_root / "kopia-logs"),
            "KOPIA_CHECK_FOR_UPDATES": "false",
        }
        tools.commands.run(
            [executable, "repository", "create", "filesystem", "--path", repo, "--no-persist-credentials", "--no-use-credential-manager", "--no-progress"],
            description=f"{lane_spec.label} init",
            env=env,
        )
        if lane_spec.arguments:
            tools.commands.run(
                [executable, "policy", "set", "--global", *lane_spec.arguments],
                description=f"{lane_spec.label} policy",
                env=env,
            )
    elif kind == "plakar":
        env = {"PLAKAR_INSECURE_PLAINTEXT": "1"}
        tools.commands.run(
            _plakar_command(executable, repo, plakar_cache_directory, "create", "-plaintext"),
            description=f"{lane_spec.label} init",
            env=env,
        )
    else:
        env = {
            "BORG_BASE_DIR": str(lane_root / "borg"),
            "BORG_CACHE_DIR": str(lane_root / "borg-cache"),
        }
        tools.commands.run([executable, "repo-create", "--repo", repo, "--encryption", "none-blake3"], description=f"{lane_spec.label} init", env=env)

    bulk = lane_spec.kind.endswith("exact-bulk")
    started = time.perf_counter()
    identifiers: dict[str, str] = {}
    if bulk and kind == "restic":
        result = tools.commands.run(
            [executable, "backup", *lane_spec.arguments, "--json", "--repo", repo, *[source.name for source in sources]],
            description=f"{lane_spec.label} backup",
            cwd=product,
            env=env,
        )
        identifiers["bulk"] = _restic_id(result.stdout)
    elif bulk and kind == "kopia":
        tools.commands.run([executable, "snapshot", "create", product, "--no-progress"], description=f"{lane_spec.label} create", env=env)
        identifiers["bulk"] = product.name
    elif bulk and kind == "plakar":
        tools.commands.run(
            _plakar_command(executable, repo, plakar_cache_directory, "backup", *lane_spec.arguments, "-cache", "no", "-no-progress", *[source.name for source in sources]),
            description=f"{lane_spec.label} backup",
            cwd=product,
            env=env,
        )
        identifiers["bulk"] = "latest"
    elif bulk:
        archive = f"{lane_spec.label}-bulk"
        tools.commands.run(
            [executable, "create", *lane_spec.arguments, "--repo", repo, archive, *[source.name for source in sources]],
            description=f"{lane_spec.label} create",
            cwd=product,
            env=env,
        )
        identifiers["bulk"] = archive
    else:
        for index, source in enumerate(sources, start=1):
            if kind == "restic":
                result = tools.commands.run(
                    [executable, "backup", *lane_spec.arguments, "--json", "--repo", repo, source.name],
                    description=f"{lane_spec.label} backup {source.name}",
                    cwd=product,
                    env=env,
                )
                identifiers[source.name] = _restic_id(result.stdout)
            elif kind == "kopia":
                tools.commands.run([executable, "snapshot", "create", source, "--no-progress"], description=f"{lane_spec.label} create {source.name}", env=env)
                identifiers[source.name] = source.name
            elif kind == "plakar":
                tag = f"gutch-benchmark-{index:04d}"
                tools.commands.run(
                    _plakar_command(executable, repo, plakar_cache_directory, "backup", *lane_spec.arguments, "-cache", "no", "-no-progress", "-tag", tag, source.name),
                    description=f"{lane_spec.label} backup {source.name}",
                    cwd=product,
                    env=env,
                )
                identifiers[source.name] = tag
            else:
                archive = f"{lane_spec.label}-{source.name}"
                tools.commands.run(
                    [executable, "create", *lane_spec.arguments, "--repo", repo, archive, source.name],
                    description=f"{lane_spec.label} create {source.name}",
                    cwd=product,
                    env=env,
                )
                identifiers[source.name] = archive
    backup = time.perf_counter() - started
    repo_stats = scan_tree(repo)

    def restore_output() -> float:
        restore_started = time.perf_counter()
        output.mkdir(parents=True, exist_ok=True)
        if bulk and kind == "restic":
            tools.commands.run(
                [executable, "restore", "latest", "--repo", repo, "--target", output],
                description=f"{lane_spec.label} restore",
                env=env,
            )
        elif bulk and kind == "kopia":
            tools.commands.run(
                [executable, "snapshot", "restore", product, output, "--snapshot-time=latest", "--no-progress"],
                description=f"{lane_spec.label} restore",
                env=env,
            )
        elif bulk and kind == "plakar":
            tools.commands.run(
                _plakar_command(executable, repo, plakar_cache_directory, "restore", "-to", output),
                description=f"{lane_spec.label} restore",
                env=env,
            )
        elif bulk:
            tools.commands.run(
                [executable, "extract", "--repo", repo, identifiers["bulk"]],
                description=f"{lane_spec.label} extract",
                cwd=output,
                env=env,
            )
        else:
            for source in reversed(sources):
                if kind == "restic":
                    tools.commands.run(
                        [executable, "restore", identifiers[source.name], "--repo", repo, "--target", output, "--include", source.name],
                        description=f"{lane_spec.label} restore {source.name}",
                        env=env,
                    )
                elif kind == "kopia":
                    tools.commands.run(
                        [executable, "snapshot", "restore", source, output / source.name, "--snapshot-time=latest", "--no-progress"],
                        description=f"{lane_spec.label} restore {source.name}",
                        env=env,
                    )
                elif kind == "plakar":
                    destination = output / source.name
                    destination.mkdir(parents=True, exist_ok=True)
                    tools.commands.run(
                        _plakar_command(executable, repo, plakar_cache_directory, "restore", "-to", destination, "-tag", identifiers[source.name]),
                        description=f"{lane_spec.label} restore {source.name}",
                        env=env,
                    )
                else:
                    tools.commands.run(
                        [executable, "extract", "--repo", repo, identifiers[source.name]],
                        description=f"{lane_spec.label} extract {source.name}",
                        cwd=output,
                        env=env,
                    )
        return time.perf_counter() - restore_started

    restore = restore_output()
    corrupt_seconds = 0.0
    repair_seconds = 0.0
    if recovery_enabled:
        recovery_destinations = [output] if bulk else [output / source.name for source in reversed(sources)]
        plans = [
            build_corruption_plan(
                destination,
                lane_root / "stats" / f"corruption-{index:03d}.json",
                index,
            )
            for index, destination in enumerate(recovery_destinations, start=1)
        ]
        corrupt_started = time.perf_counter()
        for destination, plan in zip(recovery_destinations, plans, strict=True):
            apply_corruption_plan(destination, plan)
        corrupt_seconds = time.perf_counter() - corrupt_started
        remove_tree(output)
        repair_seconds = restore_output()

    verify_started = time.perf_counter()
    if kind == "restic":
        tools.commands.run([executable, "check", "--read-data", "--repo", repo], description=f"{lane_spec.label} check", env=env)
    elif kind == "kopia":
        verify_sources = [product] if bulk else sources
        for source in verify_sources:
            tools.commands.run(
                [executable, "snapshot", "verify", "--sources", source, "--verify-files-percent=100", "--no-progress"],
                description=f"{lane_spec.label} verify {source.name}",
                env=env,
            )
    elif kind == "plakar":
        tools.commands.run(
            _plakar_command(executable, repo, plakar_cache_directory, "check"),
            description=f"{lane_spec.label} check",
            env=env,
        )
    else:
        tools.commands.run([executable, "check", "--repo", repo, "--verify-data"], description=f"{lane_spec.label} check", env=env)
    verify = time.perf_counter() - verify_started
    differences = _compare(sources, output)
    return LaneResult(
        lane_spec.label,
        lane_spec.kind,
        {
            "backup": backup,
            "restore": restore,
            "verify": verify,
            "compare": 0.0 if not differences else 1.0,
            "corrupt": corrupt_seconds,
            "repair": repair_seconds,
            "growth": repo_stats.total_bytes,
            "repo_size": repo_stats.total_bytes,
            "repo_files": repo_stats.file_count,
        },
        {},
        {
            "verify_invocations": 1 if bulk or kind != "kopia" else len(sources),
            "verify_position": "after_recovery",
            "corrupt_repair_after_reconstruct": recovery_enabled,
            "differences": differences,
        },
    )
