"""Local orchestrator for fixed-dataset remote Gutch benchmarks."""

from __future__ import annotations

import json
from pathlib import Path
import secrets
import tempfile
import zipfile

import yaml

from ..benchmark_config import BenchmarkConfig, load_benchmark_config
from ..common.artifacts import safe_component
from ..common.traces import open_trace_in_perfetto
from ..gutch_fixture_runner.s3_cycle import load_s3_cycle
from .launcher import RemoteLauncher, RemoteTarget


BENCHMARK_ROOT = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = BENCHMARK_ROOT.parents[2]
REMOTE_ENTRY = Path(__file__).with_name("entry.py")


def _package_sources(destination: Path) -> None:
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as package:
        for path in BENCHMARK_ROOT.rglob("*"):
            if not path.is_file() or "bin" in path.parts or "__pycache__" in path.parts:
                continue
            if path.suffix not in {".py", ".yaml"}:
                continue
            package.write(path, path.relative_to(REPOSITORY_ROOT).as_posix())
        dataset_script = REPOSITORY_ROOT / "gutch_repo" / "tests" / "prepare_test_dataset.py"
        package.write(dataset_script, "prepare_test_dataset.py")
        recipe = REPOSITORY_ROOT / "gutch_repo" / "tests" / "test_dataset_recipes" / "blender-official.json"
        package.write(recipe, "gutch_repo/tests/test_dataset_recipes/blender-official.json")


def _runtime_config(config: BenchmarkConfig, path: Path, remote_workspace: str) -> None:
    values = yaml.safe_load(config.path.read_text(encoding="utf-8")) or {}
    values["local"] = {
        "product_root": f"{config.remote_fixtures_root}/{Path(remote_workspace).name}",
        "artifacts_root": f"{remote_workspace}/artifacts",
    }
    values["remote"] = {
        "default_target": config.remote_target,
        "workspace_root": config.remote_workspace_root,
        "fixtures_root": config.remote_fixtures_root,
        "python": config.remote_python,
    }
    values["ssh"] = {}
    path.write_text(yaml.safe_dump(values, sort_keys=False), encoding="utf-8")


def _remote_path(workspace: str, name: str) -> str:
    return f"{workspace.rstrip('/')}/{name}"


def run_remote_benchmark(
    *,
    fixture: str,
    config_path: Path | None = None,
    artifacts_root: Path | None = None,
    keep_artifacts: bool = False,
) -> Path:
    config = load_benchmark_config(config_path)
    if config.hostapp_cli_linux_x64 is None:
        raise ValueError(
            "benchmark config requires binaries.hostapp_cli_linux_x64 with version, url, and sha256"
        )
    fixture_path = BENCHMARK_ROOT / "gutch_fixture_runner" / "fixtures" / f"{fixture}.yaml"
    load_s3_cycle(fixture_path)
    target = RemoteTarget.from_config(config_path=config.path)
    launcher = RemoteLauncher(target)
    workspace = f"{target.workspace.rstrip('/')}/run-{secrets.token_hex(12)}"
    local_root = (artifacts_root or config.artifacts_root) / "remote" / safe_component(fixture)
    local_root.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="gutch-remote-") as temporary:
        temporary_root = Path(temporary)
        package_path = temporary_root / "benchmark-package.zip"
        runtime_config_path = temporary_root / "config.yml"
        _package_sources(package_path)
        _runtime_config(config, runtime_config_path, workspace)

        remote_package = _remote_path(workspace, "benchmark-package.zip")
        remote_config = _remote_path(workspace, "config.yml")
        remote_entry = _remote_path(workspace, "entry.py")
        launcher.run(
            ["mkdir", "-p", workspace],
            description="create remote benchmark workspace",
        )
        try:
            launcher.upload(package_path, remote_package, description="upload benchmark package")
            launcher.upload(runtime_config_path, remote_config, description="upload benchmark config")
            launcher.upload(REMOTE_ENTRY, remote_entry, description="upload remote benchmark runner")
            command = [
                config.remote_python,
                remote_entry,
                "--package",
                remote_package,
                "--config",
                remote_config,
                "--workspace",
                workspace,
                "--fixture",
                fixture,
            ]
            if keep_artifacts:
                command.append("--keep-artifacts")
            result = launcher.run(command, description=f"run remote Gutch fixture {fixture}")
            if result.stdout:
                print(result.stdout, end="")
            if result.stderr:
                print(result.stderr, end="")

            manifest_path = local_root / "remote-result.json"
            results_path = local_root / "results.json"
            trace_path = local_root / "trace.json"
            launcher.download(
                _remote_path(workspace, "remote-result.json"),
                manifest_path,
                description="download remote benchmark manifest",
            )
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            launcher.download(manifest["results_path"], results_path, description="download remote results")
            launcher.download(manifest["trace_path"], trace_path, description="download remote Perfetto trace")
            print(f"Remote benchmark results: {results_path}")
            print(f"Remote benchmark trace: {trace_path}")
            if load_benchmark_fixture_open_perfetto(manifest["fixture"]):
                open_trace_in_perfetto(trace_path, print)
            return results_path
        finally:
            if not keep_artifacts:
                launcher.run(
                    [config.remote_python, "-c", f"import shutil; shutil.rmtree({workspace!r}, ignore_errors=True)"],
                    description="clean remote benchmark workspace",
                )


def load_benchmark_fixture_open_perfetto(fixture: str) -> bool:
    fixture_path = BENCHMARK_ROOT / "gutch_fixture_runner" / "fixtures" / f"{fixture}.yaml"
    values = yaml.safe_load(fixture_path.read_text(encoding="utf-8")) or {}
    return bool(values.get("open_perfetto", False))
