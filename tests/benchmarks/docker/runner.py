"""Local orchestrator for fixed-dataset container Gutch benchmarks."""

from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import zipfile

import yaml

from tests.benchmarks.benchmark_config import BenchmarkConfig, load_benchmark_config
from comparison_benchmark_suite.support.artifacts import create_run_dir
from comparison_benchmark_suite.support.traces import open_trace_in_perfetto
from tests.benchmarks.comparison.config import load_fixture
from tests.benchmarks.comparison.s3_cycle import load_s3_cycle
from tests.benchmarks.docker.launcher import DockerLauncher, DockerTarget


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
        from tools.build_image import benchmark_dependency_files

        for path, relative in benchmark_dependency_files():
            package.write(path, relative.as_posix())
        package.write(REPOSITORY_ROOT / "tests/__init__.py", "tests/__init__.py")
        for name in ("__init__.py", "artifacts.py"):
            package.write(REPOSITORY_ROOT / "tools" / name, "tools/" + name)
        package.write(
            REPOSITORY_ROOT / "tests/datasets/__init__.py", "tests/datasets/__init__.py"
        )
        package.write(
            REPOSITORY_ROOT / "tests/datasets/prepare_blender.py",
            "tests/datasets/prepare_blender.py",
        )
        dataset_script = REPOSITORY_ROOT / "tests/datasets/generate.py"
        package.write(dataset_script, "tests/datasets/generate.py")
        recipe = REPOSITORY_ROOT / "tests/datasets/corpus/blender/case.json"
        package.write(recipe, "tests/datasets/corpus/blender/case.json")


def _runtime_config(
    config: BenchmarkConfig, path: Path, container_workspace: str
) -> None:
    values = yaml.safe_load(config.path.read_text(encoding="utf-8")) or {}
    values["local"] = {
        "product_root": f"/tmp/gutch-benchmark/dataset/{CONTAINER_DATASET_DIRECTORY}/Blender",
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
    fixture_path = BENCHMARK_ROOT / "comparison" / "fixtures" / f"{fixture}.yaml"
    fixture_spec = load_fixture(fixture_path)
    if fixture_spec.runner == "gutch-s3":
        load_s3_cycle(fixture_path)
    elif fixture_spec.runner != "gutch-fixture":
        raise ValueError(
            f"fixture {fixture!r} is not a supported container Gutch fixture"
        )
    target = DockerTarget.start(config, REPOSITORY_ROOT)
    try:
        return _execute_benchmark(
            target,
            config,
            fixture,
            artifacts_root,
            transfer_concurrency,
            transfer_max_concurrency,
            keep_artifacts,
        )
    finally:
        target.close()


def _execute_benchmark(
    target,
    config,
    fixture,
    artifacts_root,
    transfer_concurrency,
    transfer_max_concurrency,
    keep_artifacts,
):
    launcher = DockerLauncher(target)
    workspace = target.workspace
    local_root = create_run_dir(
        (artifacts_root or config.artifacts_root) / "container", "linux", fixture
    )
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
        launcher.upload(
            package_path, container_package, description="upload benchmark package"
        )
        launcher.upload(
            runtime_config_path, container_config, description="upload benchmark config"
        )
        launcher.upload(
            CONTAINER_ENTRY,
            container_entry,
            description="upload container benchmark runner",
        )
        launcher.run(
            [
                "cp",
                "/gutch/.builds/compiler-cache/cargo/release/gutch-cli",
                container_binary,
            ],
            description="copy Docker-built Linux gutch-cli",
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
            command.extend(
                ["--transfer-max-concurrency", str(transfer_max_concurrency)]
            )
        if keep_artifacts:
            command.append("--keep-artifacts")
        try:
            launcher.run(
                command,
                description=f"run container Gutch fixture {fixture}",
                on_output=_print_container_output,
            )
        except BaseException:
            try:
                launcher.download(
                    workspace,
                    local_root / "failed-container",
                    description="preserve failed benchmark evidence",
                )
            except Exception as error:
                print(
                    f"Could not export failed benchmark evidence: {error}",
                    file=sys.stderr,
                )
            raise

        manifest_path = local_root / "container-result.json"
        results_path = local_root / "results.json"
        trace_path = local_root / "trace.json"
        launcher.download(
            _container_path(workspace, "container-result.json"),
            manifest_path,
            description="download container benchmark manifest",
        )
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        launcher.download(
            manifest["results_path"],
            results_path,
            description="download container results",
        )
        launcher.download(
            manifest["trace_path"],
            trace_path,
            description="download container Perfetto trace",
        )
        evidence_path = local_root / "evidence.zip"
        launcher.download(
            manifest["evidence_path"],
            evidence_path,
            description="download complete benchmark evidence",
        )
        with zipfile.ZipFile(evidence_path) as archive:
            for member in archive.infolist():
                target_path = (local_root / member.filename).resolve()
                if not target_path.is_relative_to(local_root.resolve()):
                    raise ValueError("benchmark evidence contains an unsafe path")
            archive.extractall(local_root)
        print(f"Container benchmark results: {results_path}")
        print(f"Container benchmark trace: {trace_path}")
        if load_benchmark_fixture_open_perfetto(manifest["fixture"]):
            open_trace_in_perfetto(trace_path, print)
        return results_path


def load_benchmark_fixture_open_perfetto(fixture: str) -> bool:
    fixture_path = BENCHMARK_ROOT / "comparison" / "fixtures" / f"{fixture}.yaml"
    values = yaml.safe_load(fixture_path.read_text(encoding="utf-8")) or {}
    return bool(values.get("open_perfetto", False))


def run_docker_benchmark(**kwargs):
    return _run_docker_benchmark(**kwargs)
