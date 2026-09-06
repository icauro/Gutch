"""Local orchestrator for fixed-dataset container Gutch benchmarks."""

from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import zipfile

import yaml

from ..benchmark_config import BenchmarkConfig, load_benchmark_config
from ..common.artifacts import safe_component
from ..common.traces import open_trace_in_perfetto
from ..gutch_fixture_runner.config import load_fixture
from ..gutch_fixture_runner.external_tools import ensure_linux_external_tools
from ..gutch_fixture_runner.s3_cycle import load_s3_cycle
from .launcher import DockerLauncher, DockerTarget


BENCHMARK_ROOT = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = BENCHMARK_ROOT.parents[1]
CONTAINER_ENTRY = Path(__file__).with_name("entry.py")
CONTAINER_DATASET_DIRECTORY = "gutch-container-blender-cache"


def _package_sources(destination: Path) -> None:
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as package:
        for path in BENCHMARK_ROOT.rglob("*"):
            if not path.is_file() or "bin" in path.parts or "__pycache__" in path.parts:
                continue
            if path.suffix not in {".py", ".yaml"}:
                continue
            package.write(path, path.relative_to(REPOSITORY_ROOT).as_posix())
        dataset_script = REPOSITORY_ROOT / "gutch_repo" / "test_dataset" / "ensure_test_dataset.py"
        package.write(dataset_script, "gutch_repo/test_dataset/ensure_test_dataset.py")
        recipe = REPOSITORY_ROOT / "gutch_repo" / "test_dataset" / "test_dataset_recipes" / "blender-official.json"
        package.write(recipe, "gutch_repo/test_dataset/test_dataset_recipes/blender-official.json")


def _runtime_config(config: BenchmarkConfig, path: Path, container_workspace: str) -> None:
    values = yaml.safe_load(config.path.read_text(encoding="utf-8")) or {}
    values["local"] = {
        "product_root": f"/tmp/hostapp-benchmark/dataset/{CONTAINER_DATASET_DIRECTORY}/Blender",
        "artifacts_root": f"{container_workspace}/artifacts",
    }
    values.pop("remote", None)
    values.pop("ssh", None)
    values["environment"] = config.environment
    path.write_text(yaml.safe_dump(values, sort_keys=False), encoding="utf-8")


def _container_path(workspace: str, name: str) -> str:
    return f"{workspace.rstrip('/')}/{name}"


def _print_container_output(line: str, is_stderr: bool) -> None:
    text = line.rstrip()
    if not text:
        return
    if text.startswith("CONTAINER_PROGRESS "):
        try:
            progress = json.loads(text.removeprefix("CONTAINER_PROGRESS "))
            stage = progress["stage"]
            detail = progress["detail"]
            print(f"[container] {stage}: {detail}", file=sys.stderr, flush=True)
            return
        except (json.JSONDecodeError, KeyError, TypeError):
            pass
    print(text, file=sys.stderr if is_stderr else sys.stdout, flush=True)


def _build_linux_cli(repo_root: Path) -> Path:
    from scripts.build.launcher._linux_builder import build_linux_gutch
    return build_linux_gutch(repo_root)


def _run_docker_benchmark(
    *,
    fixture: str,
    config_path: Path | None = None,
    artifacts_root: Path | None = None,
    transfer_concurrency: int | None = None,
    transfer_max_concurrency: int | None = None,
    keep_artifacts: bool = False,
) -> Path:
    config = load_benchmark_config(config_path)
    fixture_path = BENCHMARK_ROOT / "gutch_fixture_runner" / "fixtures" / f"{fixture}.yaml"
    fixture_spec = load_fixture(fixture_path)
    if fixture_spec.runner == "gutch-s3":
        load_s3_cycle(fixture_path)
    elif fixture_spec.runner != "gutch-fixture":
        raise ValueError(f"fixture {fixture!r} is not a supported container Gutch fixture")
    external_kinds = {
        lane.kind.split("-", 1)[0]
        for lane in fixture_spec.lanes
        if lane.kind.startswith(("restic-", "kopia-", "borg-", "plakar-"))
    }
    linux_external_tools = ensure_linux_external_tools(REPOSITORY_ROOT, external_kinds)
    binary_path = _build_linux_cli(REPOSITORY_ROOT)
    target = DockerTarget.start(config, REPOSITORY_ROOT)
    launcher = DockerLauncher(target)
    workspace = target.workspace
    local_root = (artifacts_root or config.artifacts_root) / "container" / safe_component(fixture)
    local_root.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="gutch-docker-") as temporary:
        temporary_root = Path(temporary)
        package_path = temporary_root / "benchmark-package.zip"
        runtime_config_path = temporary_root / "config.yml"
        _package_sources(package_path)
        _runtime_config(config, runtime_config_path, workspace)

        container_package = _container_path(workspace, "benchmark-package.zip")
        container_config = _container_path(workspace, "config.yml")
        container_entry = _container_path(workspace, "entry.py")
        container_binary = _container_path(workspace, "gutch-cli")
        launcher.run(
            ["mkdir", "-p", workspace],
            description="create container benchmark workspace",
        )
        container_external_root = _container_path(workspace, "external")
        if linux_external_tools:
            launcher.run(
                ["mkdir", "-p", container_external_root],
                description="create container external-tool directory",
            )
        launcher.upload(package_path, container_package, description="upload benchmark package")
        launcher.upload(runtime_config_path, container_config, description="upload benchmark config")
        launcher.upload(CONTAINER_ENTRY, container_entry, description="upload container benchmark runner")
        launcher.upload(binary_path, container_binary, description="upload Docker-built Linux gutch-cli")
        for kind, path in sorted(linux_external_tools.items()):
            launcher.upload(
                path,
                _container_path(container_external_root, kind),
                description=f"upload Linux {kind} benchmark tool",
            )
        command = [
            "python3",
            container_entry,
            "--package",
            container_package,
            "--config",
            container_config,
            "--workspace",
            workspace,
            "--fixture",
            fixture,
            "--binary",
            container_binary,
        ]
        if transfer_concurrency is not None:
            command.extend(["--transfer-concurrency", str(transfer_concurrency)])
        if transfer_max_concurrency is not None:
            command.extend(["--transfer-max-concurrency", str(transfer_max_concurrency)])
        if linux_external_tools:
            command.extend(["--external-root", container_external_root])
        if keep_artifacts:
            command.append("--keep-artifacts")
        launcher.run(
            command,
            description=f"run container Gutch fixture {fixture}",
            on_output=_print_container_output,
        )

        manifest_path = local_root / "container-result.json"
        results_path = local_root / "results.json"
        trace_path = local_root / "trace.json"
        launcher.download(
            _container_path(workspace, "container-result.json"),
            manifest_path,
            description="download container benchmark manifest",
        )
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        launcher.download(manifest["results_path"], results_path, description="download container results")
        launcher.download(manifest["trace_path"], trace_path, description="download container Perfetto trace")
        print(f"Container benchmark results: {results_path}")
        print(f"Container benchmark trace: {trace_path}")
        if load_benchmark_fixture_open_perfetto(manifest["fixture"]):
            open_trace_in_perfetto(trace_path, print)
        return results_path


def load_benchmark_fixture_open_perfetto(fixture: str) -> bool:
    fixture_path = BENCHMARK_ROOT / "gutch_fixture_runner" / "fixtures" / f"{fixture}.yaml"
    values = yaml.safe_load(fixture_path.read_text(encoding="utf-8")) or {}
    return bool(values.get("open_perfetto", False))



def run_docker_benchmark(**kwargs):
    from tests.common import _test_environment as environments
    success = False
    try:
        result = _run_docker_benchmark(**kwargs)
        success = True
        return result
    finally:
        if kwargs.get('keep_artifacts'):
            environments.retain_environment()
        environments.finish_environments(success)
