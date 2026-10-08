import io
import json
import os
import shutil
import sys
from pathlib import Path
from types import SimpleNamespace
import zipfile

import comparison_benchmark_suite.support.command as command_module
from comparison_benchmark_suite.support.artifacts import RunLayout
from comparison_benchmark_suite.support.command import CommandResult, CommandRunner
from comparison_benchmark_suite.scheduling.config import ScenarioCatalog
from tests.benchmarks.scheduling.environment import load_benchmark_environment
from comparison_benchmark_suite.support.traces import combine_labeled_traces
from tests.benchmarks.benchmark_config import load_benchmark_config
import tests.benchmarks.comparison.cli as gutch_cli
import tests.benchmarks.comparison.binary as gutch_binary
from comparison_benchmark_suite.support.commands import GutchCli
from tests.benchmarks.comparison.config import FixtureCatalog, load_fixture
import tests.benchmarks.comparison.__main__ as gutch_launcher
from tests.benchmarks.comparison.__main__ import _prompt_product, main as gutch_main
from tests.benchmarks.comparison.binary import resolve_gutch_executable
from comparison_benchmark_suite.tools.external import ExternalTools, _plakar_cache_directory, _plakar_command, run_external
from tests.benchmarks.support.s3 import S3Settings, s3_key
from tests.benchmarks.comparison.s3_cycle import load_s3_cycle
from comparison_benchmark_suite.tools.models import FixtureSpec, LaneResult, LaneSpec
from tests.benchmarks.comparison.report import print_gutch_profile_table
import tests.benchmarks.comparison.workflow as gutch_workflow
import comparison_benchmark_suite.scheduling.runner as benchmark_runner
from comparison_benchmark_suite.tools.workflow import LocalFixtureRunner
from tests.benchmarks.docker.launcher import DockerLauncher, DockerTarget
import tests.benchmarks.docker.runner as container_runner
import tests.benchmarks.docker.__main__ as container_launcher
import tests.benchmarks.docker.entry as container_entry
from tests.benchmarks.s3_throughput.config import load_storage_scenario
from tests.benchmarks.s3_throughput.__main__ import main as storage_main
import tests.benchmarks.s3_throughput.workflow as storage_workflow
import tests.benchmarks.s3_throughput.cli as storage_cli
from tests.benchmarks.s3_throughput.config import StorageTarget


def test_catalog_contains_all_gutch_benchmark_scenarios() -> None:
    names = {
        document.name
        for document in ScenarioCatalog(Path("tests/benchmarks/comparison/fixtures")).list()
    }
    assert {
        "07_gutch_default_trace_with_corrupt_repair",
        "03_gutch_batch_vs_individual_capture",
        "04_gutch_chunk_size_profiles",
        "05_gutch_compression_profiles",
        "01_filesystem_copy_baseline",
        "14_gutch_reconstruction_storage_paths",
        "15_gutch_s3_cold_publication_cycle",
        "02_gutch_storage_layout_baseline",
        "06_gutch_worker_count_profiles",
    } <= names


def test_internal_fixture_catalog_excludes_external_tools():
    fixtures = FixtureCatalog(Path("tests/benchmarks/comparison/fixtures")).list()
    assert fixtures
    assert all(lane.tool in {"gutch", "filesystem"} for fixture in fixtures for lane in fixture.lanes)


def test_internal_runner_rejects_external_tools_before_creating_artifacts(tmp_path):
    import pytest
    fixture = load_fixture(Path(__file__).resolve().parents[3] / "comparison_benchmark_suite/fixtures/blender/fixture.yaml")
    artifacts = tmp_path / "runs"
    with pytest.raises(ValueError, match="standalone suite"):
        gutch_workflow.run_fixture(fixture, tmp_path, GutchCli(Path("unused")), artifacts, keep_artifacts=False)
    assert not artifacts.exists()


def test_fixture_waits_default_to_four_seconds() -> None:
    fixture = FixtureCatalog(Path(__file__).resolve().parents[1] / "benchmarks/comparison/fixtures").resolve("02_gutch_storage_layout_baseline")
    assert fixture.lane_wait_seconds == 4.0
    assert fixture.step_wait_seconds == 4.0


def test_external_tools_use_explicit_binary(tmp_path: Path) -> None:
    executable = tmp_path / "plakar"
    executable.write_bytes(b"plakar")
    assert ExternalTools(CommandRunner(), plakar=executable).executable("plakar") == executable


def test_kopia_init_uses_supported_keyring_flag(tmp_path: Path) -> None:
    product = tmp_path / "product"
    source = product / "1.0.0-linux-x64-public"
    source.mkdir(parents=True)
    (source / "payload.bin").write_bytes(b"payload")
    executable = tmp_path / "kopia"
    executable.write_bytes(b"kopia")

    class FakeCommands:
        def __init__(self) -> None:
            self.calls: list[tuple[str, ...]] = []

        def run(self, command, *, description, cwd=None, env=None, expected_failure=False, on_output=None):
            rendered = tuple(str(item) for item in command)
            self.calls.append(rendered)
            if rendered[1:4] == ("repository", "create", "filesystem"):
                Path(rendered[rendered.index("--path") + 1]).mkdir(parents=True, exist_ok=True)
            elif rendered[1:3] == ("snapshot", "restore"):
                shutil.copytree(rendered[3], rendered[4])
            return CommandResult(rendered, 0.01, 0, "", "")

    commands = FakeCommands()
    lane_spec = LaneSpec(
        label="kopia",
        tool="kopia",
        profile=FixtureCatalog(Path(__file__).resolve().parents[1] / "benchmarks/comparison/fixtures").resolve("07_gutch_default_trace_with_corrupt_repair").lanes[0].profile,
    )
    result = run_external(
        ExternalTools(commands, kopia=executable),
        RunLayout(tmp_path / "run").lane("kopia"),
        product,
        lane_spec,
    )

    assert result.metadata["snapshot_metrics"][source.name] == {"input_bytes": 7, "backup": 0.01, "restore": 0.01}
    init = commands.calls[0]
    assert "--no-use-keyring" not in init
    assert "--no-use-credential-manager" not in init


def test_borg_lane_uses_supported_repository_syntax(tmp_path: Path) -> None:
    product = tmp_path / "product"
    source = product / "1.0.0-linux-x64-public"
    source.mkdir(parents=True)
    (source / "payload.bin").write_bytes(b"payload")
    executable = tmp_path / "borg"
    executable.write_bytes(b"borg")

    class FakeCommands:
        def __init__(self) -> None:
            self.calls: list[tuple[str, ...]] = []
            self.archives: dict[str, Path] = {}

        def run(self, command, *, description, cwd=None, env=None, expected_failure=False, on_output=None):
            rendered = tuple(str(item) for item in command)
            self.calls.append(rendered)
            if rendered[1] == "repo-create":
                Path(rendered[rendered.index("--repo") + 1]).mkdir(parents=True, exist_ok=True)
            elif rendered[1] == "create":
                archive = rendered[-2]
                self.archives[archive] = Path(cwd) / rendered[-1]
            elif rendered[1] == "extract":
                source_path = self.archives[rendered[-1]]
                shutil.copytree(source_path, Path(cwd) / source_path.name)
            return CommandResult(rendered, 0.01, 0, "borg 2.0.0b23" if rendered[1] == "--version" else "", "")

    commands = FakeCommands()
    lane_spec = LaneSpec(
        label="borg",
        tool="borg",
        profile=FixtureCatalog(Path(__file__).resolve().parents[1] / "benchmarks/comparison/fixtures").resolve("07_gutch_default_trace_with_corrupt_repair").lanes[0].profile,
        arguments=("--compression", "zstd,1"),
    )
    run_external(
        ExternalTools(commands, borg=executable),
        RunLayout(tmp_path / "run").lane("borg"),
        product,
        lane_spec,
    )

    assert commands.calls[1][1:] == ("repo-create", "--repo", str(tmp_path / "run/lanes/borg/payload/repo"), "--encryption", "none-sha256")
    assert all("--repo" in call for call in commands.calls[1:])


def test_plakar_uses_short_unique_cache_socket_path(tmp_path: Path) -> None:
    cache_directory = _plakar_cache_directory(tmp_path / "long" / "benchmark" / "lane")
    command = _plakar_command(Path("plakar.exe"), tmp_path / "repo", cache_directory, "ls")

    assert command[1] == "-quiet"
    assert "-disable-security-check" not in command
    assert command[command.index("-cachedir") + 1] == cache_directory
    assert len(str(cache_directory / "cached.sock")) < 108


def test_fixture_waits_are_configurable_and_non_negative(tmp_path: Path) -> None:
    fixture = tmp_path / "waits.yaml"
    fixture.write_text(
        "\n".join(
            [
                "name: waits",
                "runner: gutch-fixture",
                "lane_wait_seconds: 1.5",
                "step_wait_seconds: 0",
                "lanes:",
                "  - label: lane",
                "    tool: gutch",
            ]
        ),
        encoding="utf-8",
    )
    loaded = load_fixture(fixture)
    assert loaded.lane_wait_seconds == 1.5
    assert loaded.step_wait_seconds == 0.0

    fixture.write_text(fixture.read_text(encoding="utf-8").replace("step_wait_seconds: 0", "step_wait_seconds: -1"), encoding="utf-8")
    try:
        load_fixture(fixture)
    except ValueError as error:
        assert "step_wait_seconds" in str(error)
    else:
        raise AssertionError("negative step wait should be rejected")


def test_benchmark_environment_is_in_yaml_and_preserves_process_values(tmp_path: Path, monkeypatch) -> None:
    config_path = tmp_path / "config.yml"
    config_path.write_text(
        "\n".join(
            [
                "local:",
                "  product_root: products",
                "  artifacts_root: artifacts",
                "environment:",
                "  GUTCH_S3_BUCKET_NAME: file-bucket",
                "  GUTCH_S3_ENDPOINT: http://file-endpoint",
                "container:",
                "  default_target: test-server",
                "  workspace_root: /tmp/benchmark",
                "  fixtures_root: /srv/fixtures",
                "  python: python3",
                "ssh:",
                "  test-server:",
                "    host: example.test",
                "    user: tester",
            ]
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("GUTCH_S3_BUCKET_NAME", "process-bucket")

    loaded = load_benchmark_environment(config_path)

    assert loaded == config_path.resolve()
    assert S3Settings.from_environment().bucket == "process-bucket"
    assert S3Settings.from_environment().endpoint == "http://file-endpoint"


def test_s3_settings_use_fixture_values_when_environment_is_absent(monkeypatch) -> None:
    monkeypatch.delenv("GUTCH_S3_BUCKET_NAME", raising=False)
    monkeypatch.delenv("GUTCH_S3_ENDPOINT", raising=False)

    settings = gutch_cli._s3_settings(
        SimpleNamespace(
            bucket=None,
            endpoint=None,
            region=None,
            access_key_id=None,
            secret_access_key=None,
        ),
        {
            "endpoint": "http://127.0.0.1:9744",
            "bucket": "gutch-tests",
            "region": "auto",
            "access_key_default": "gutch-test-access",
            "secret_key_default": "gutch-test-secret",
        },
    )

    assert settings.endpoint == "http://127.0.0.1:9744"
    assert settings.bucket == "gutch-tests"
    assert settings.access_key_id == "gutch-test-access"


def test_gutch_no_argument_launcher_is_interactive(monkeypatch, capsys) -> None:
    monkeypatch.setattr(gutch_launcher, "resolve_gutch_executable", lambda *_args: Path("gutch.exe"))
    monkeypatch.setattr("builtins.input", lambda prompt: "")
    assert gutch_main([]) == 0
    assert "Gutch benchmark interactive mode" in capsys.readouterr().out


def test_gutch_launcher_repo_root_points_to_checkout() -> None:
    assert gutch_launcher.REPO_ROOT == Path(__file__).resolve().parents[2]


def test_container_launcher_menu_uses_config_and_can_select_diagnostic_profile(monkeypatch, tmp_path: Path) -> None:
    config_path = tmp_path / "config.yml"
    events: list[dict[str, object]] = []
    monkeypatch.setattr(
        container_launcher,
        "load_benchmark_config",
        lambda _path: SimpleNamespace(path=config_path),
    )
    monkeypatch.setattr(
        container_launcher,
        "_fixtures",
        lambda: [container_launcher.ContainerFixture("gutch-s3", "fixture", "description", 4, 16)],
    )
    answers = iter(("1", "2"))
    monkeypatch.setattr("builtins.input", lambda _prompt: next(answers))
    monkeypatch.setattr(
        container_launcher,
        "run_docker_benchmark",
        lambda **kwargs: events.append(kwargs),
    )

    assert container_launcher.main([]) == 0
    assert events == [
        {
            "fixture": "fixture",
            "config_path": config_path,
            "transfer_concurrency": 1,
            "transfer_max_concurrency": 1,
        }
    ]


def test_gutch_interactive_ensures_cli_before_product_prompt(monkeypatch) -> None:
    events: list[str] = []
    monkeypatch.setattr(
        gutch_launcher,
        "resolve_gutch_executable",
        lambda *_args: events.append("ensure") or Path("gutch.exe"),
    )
    monkeypatch.setattr(
        gutch_launcher,
        "_interactive_fixtures",
        lambda: [gutch_launcher.InteractiveFixture("gutch", "fixture", "")],
    )
    monkeypatch.setattr(
        gutch_launcher,
        "_prompt_choice",
        lambda *_args: events.append("choice") or "gutch/fixture",
    )
    monkeypatch.setattr(
        gutch_launcher,
        "_prompt_product",
        lambda: events.append("product") or None,
    )

    assert gutch_main([]) == 0
    assert events.index("ensure") < events.index("product")


def test_storage_no_argument_launcher_is_interactive(monkeypatch, capsys) -> None:
    monkeypatch.setattr("builtins.input", lambda prompt: "")
    assert storage_main([]) == 0
    assert "Storage benchmark interactive mode" in capsys.readouterr().out


def test_gutch_product_prompt_selects_from_lookup_root(tmp_path: Path, monkeypatch) -> None:
    products_root = tmp_path / "products"
    first = products_root / "Alpha"
    second = products_root / "Beta"
    (first / "1.0.0-windows-x86-public").mkdir(parents=True)
    (second / "2.0.0-windows-x86-public").mkdir(parents=True)
    monkeypatch.setattr(gutch_launcher, "DEFAULT_PRODUCTS_ROOT", products_root)
    monkeypatch.setattr("builtins.input", lambda prompt: "2")

    assert _prompt_product() == second


def test_s3_cycle_fixture_is_typed_and_has_no_post_reconstruct_verify() -> None:
    fixture, options = load_s3_cycle(
        Path("tests/benchmarks/comparison/fixtures/15_gutch_s3_cold_publication_cycle.yaml")
    )
    assert fixture.runner == "gutch-s3"
    assert len(fixture.lanes) == 3
    assert options.transfer_concurrency == 4
    source = Path("tests/benchmarks/comparison/s3_cycle.py").read_text(encoding="utf-8")
    assert '"snapshot",\n                "verify"' not in source


def test_container_fixture_catalog_contains_single_trace_and_s3_cycle() -> None:
    fixtures = {fixture.name: fixture for fixture in container_launcher._fixtures()}

    assert fixtures["12_gutch_container_trace_single_lane"].runner == "gutch-fixture"
    assert fixtures["15_gutch_s3_cold_publication_cycle"].runner == "gutch-s3"


def test_snapshot_limit_selects_newest_product_targets(tmp_path: Path) -> None:
    product = tmp_path / "Blender"
    for name in ("5.2.0", "5.2.1", "5.2.2"):
        (product / name).mkdir(parents=True)

    entries = gutch_workflow._selected_product_sources(product, 2)

    assert [path.name for path in entries] == ["5.2.1", "5.2.2"]


def test_fixture_loader_accepts_explicit_list_shape(tmp_path: Path) -> None:
    fixture = tmp_path / "fixture.yaml"
    fixture.write_text(
        "\n".join(
            [
                "name: explicit",
                "runner: gutch-fixture",
                "lanes:",
                "  - label: lane-a",
                "    tool: gutch",
                "    profile:",
                "      chunk: chunk_64KiB",
                "      compression:",
                "        algorithm: zstd",
                "        level: 1",
            ]
        ),
        encoding="utf-8",
    )
    loaded = load_fixture(fixture)
    assert loaded.lanes[0].label == "lane-a"
    assert loaded.lanes[0].profile.chunk_profile == "chunk_64KiB"
    assert loaded.lanes[0].profile.compression_algorithm == "zstd"
    assert loaded.lanes[0].profile.compression_level == 1


def test_fixture_loader_accepts_external_lane_arguments(tmp_path: Path) -> None:
    fixture = tmp_path / "external-arguments.yaml"
    fixture.write_text(
        "\n".join(
            [
                "name: external-arguments",
                "runner: gutch-fixture",
                "lanes:",
                "  - label: restic",
                "    tool: restic",
                "    arguments: [--compression, fastest]",
            ]
        ),
        encoding="utf-8",
    )

    loaded = load_fixture(fixture)

    assert loaded.lanes[0].arguments == ("--compression", "fastest")


def test_fixture_loader_rejects_arguments_on_gutch_lane(tmp_path: Path) -> None:
    fixture = tmp_path / "gutch-arguments.yaml"
    fixture.write_text(
        "\n".join(
            [
                "name: gutch-arguments",
                "runner: gutch-fixture",
                "lanes:",
                "  - label: gutch",
                "    tool: gutch",
                "    arguments: [--jobs, '2']",
            ]
        ),
        encoding="utf-8",
    )

    try:
        load_fixture(fixture)
    except ValueError as error:
        assert "only supported for external lanes" in str(error)
    else:
        raise AssertionError("Gutch lanes should use typed profile options")


def test_gutch_profile_uses_rust_cli_flag_names() -> None:
    profile = FixtureCatalog(Path(__file__).resolve().parents[1] / "benchmarks/comparison/fixtures").resolve("07_gutch_default_trace_with_corrupt_repair").lanes[0].profile
    args = GutchCli(Path("gutch-cli.exe")).profile_args(profile)
    assert "--compression-algorithm" in args
    assert "--compression-level" in args
    assert "--storage-mode" in args
    assert "--compression" not in args


def test_explicit_gutch_binary_does_not_fallback_silently(tmp_path: Path) -> None:
    missing = tmp_path / "missing.exe"
    try:
        resolve_gutch_executable(tmp_path, missing)
    except FileNotFoundError as error:
        assert str(missing) in str(error)
    else:
        raise AssertionError("missing explicit executable should fail")


def test_default_gutch_binary_rebuilds_when_source_is_newer(tmp_path: Path, monkeypatch) -> None:
    executable = tmp_path / ".builds" / "compiler-cache" / "cargo" / "release" / ("gutch-cli.exe" if os.name == "nt" else "gutch-cli")
    source = tmp_path / "src" / "command_api" / "args.rs"
    executable.parent.mkdir(parents=True)
    source.parent.mkdir(parents=True)
    executable.write_bytes(b"stale")
    source.write_text("current", encoding="utf-8")
    stale_time = source.stat().st_mtime - 10
    os.utime(executable, (stale_time, stale_time))

    def build_current_binary(*_args, **_kwargs) -> None:
        executable.write_bytes(b"current")
        current_time = source.stat().st_mtime + 10
        os.utime(executable, (current_time, current_time))

    monkeypatch.setattr(gutch_binary.subprocess, "run", build_current_binary)

    assert gutch_binary.resolve_gutch_executable(tmp_path) == executable
    assert executable.read_bytes() == b"current"


def test_default_fixture_enables_corruption_without_extra_verify_phase() -> None:
    fixture = FixtureCatalog(Path(__file__).resolve().parents[1] / "benchmarks/comparison/fixtures").resolve("07_gutch_default_trace_with_corrupt_repair")
    assert fixture.corrupt_repair is True
    source = Path(gutch_workflow.__file__).read_text(encoding="utf-8")
    assert '"snapshot",\n                "verify"' not in source


def test_recovery_options_are_fixture_level(tmp_path: Path) -> None:
    fixture = tmp_path / "fixture.yaml"
    fixture.write_text(
        "\n".join(
            [
                "name: invalid-lane-recovery",
                "runner: gutch-fixture",
                "lanes:",
                "  - label: lane",
                "    tool: gutch",
                "    corrupt_repair: true",
            ]
        ),
        encoding="utf-8",
    )
    try:
        load_fixture(fixture)
    except ValueError as error:
        assert "unsupported lane fields" in str(error)
    else:
        raise AssertionError("lane recovery options should be rejected")


def test_command_runner_captures_elapsed_and_output(monkeypatch) -> None:
    monkeypatch.setattr(
        CommandRunner,
        "_run_streamed",
        lambda *args, **kwargs: ("ok\n", "", 0),
    )
    result = CommandRunner().run(
        [sys.executable, "-c", "print('ok')"],
        description="test command",
    )
    assert result.returncode == 0
    assert result.stdout.strip() == "ok"
    assert result.elapsed_seconds >= 0


def test_command_runner_streams_output_and_retains_captured_text() -> None:
    events: list[tuple[str, bool]] = []
    result = CommandRunner(progress=False).run(
        [
            sys.executable,
            "-c",
            "import sys; print('container-step'); print('container-warning', file=sys.stderr)",
        ],
        description="streamed command",
        on_output=lambda line, is_stderr: events.append((line, is_stderr)),
    )

    assert result.stdout == "container-step\n"
    assert result.stderr == "container-warning\n"
    assert ("container-step\n", False) in events
    assert ("container-warning\n", True) in events


def test_command_runner_progress_shows_step_and_completion(monkeypatch) -> None:
    monkeypatch.setattr(
        CommandRunner,
        "_run_streamed",
        lambda *args, **kwargs: ("", "", 0),
    )
    stream = io.StringIO()
    result = CommandRunner(progress=True, stream=stream).run(
        [sys.executable, "-c", ""],
        description="bundled capture 1.0.0",
    )

    assert result.returncode == 0
    assert "bundled capture 1.0.0" in stream.getvalue()
    assert "[done]" in stream.getvalue()


def test_command_runner_waits_between_steps_and_resets_per_lane(monkeypatch) -> None:
    monkeypatch.setattr(
        CommandRunner,
        "_run_streamed",
        lambda *args, **kwargs: ("", "", 0),
    )
    waits: list[float] = []
    monkeypatch.setattr(command_module.time, "sleep", waits.append)
    runner = CommandRunner(progress=False, step_wait_seconds=4.0)
    runner.run([sys.executable], description="lane-a capture")
    runner.run([sys.executable], description="lane-a reconstruct")
    runner.begin_lane()
    runner.run([sys.executable], description="lane-b capture")

    assert waits == [4.0]


def test_run_layout_removes_only_payloads(tmp_path: Path) -> None:
    run = RunLayout(tmp_path / "run")
    lane = run.lane("default")
    (lane.payload / "repo").mkdir()
    (lane.payload / "repo" / "blob").write_text("payload", encoding="utf-8")
    (lane.stats / "result.json").write_text("{}", encoding="utf-8")
    run.remove_payloads()
    assert not (lane.payload / "repo").exists()
    assert (lane.stats / "result.json").exists()


def test_combine_labeled_traces_keeps_phases_in_one_perfetto_lane(tmp_path: Path) -> None:
    first = tmp_path / "first.json"
    second = tmp_path / "second.json"
    third = tmp_path / "third.json"
    output = tmp_path / "combined.json"
    event = {"name": "phase", "ph": "B", "ts": 10, "pid": 1, "tid": 2}
    first.write_text(json.dumps({"traceEvents": [event]}), encoding="utf-8")
    second.write_text(json.dumps({"traceEvents": [{**event, "ts": 20}]}), encoding="utf-8")
    third.write_text(json.dumps({"traceEvents": [{**event, "ts": 30}]}), encoding="utf-8")

    combine_labeled_traces(
        [("bundled", first), ("bundled", second), ("loose", third)],
        output,
    )

    document = json.loads(output.read_text(encoding="utf-8"))
    lane_pids = {
        item["args"]["name"]: item["pid"]
        for item in document["traceEvents"]
        if item.get("name") == "process_name"
    }
    phase_events = [item for item in document["traceEvents"] if item.get("name") == "phase"]
    assert sum(item["pid"] == lane_pids["bundled"] for item in phase_events) == 2
    assert sum(item["pid"] == lane_pids["loose"] for item in phase_events) == 1
    assert lane_pids["bundled"] != lane_pids["loose"]


def test_storage_fixture_and_shared_boundaries() -> None:
    scenario = load_storage_scenario(
        Path("tests/benchmarks/s3_throughput/fixtures/s3_object_storage_8mib.yaml")
    )
    assert scenario.size_bytes == 8 * 1024 * 1024
    assert s3_key("benchmarks/set", Path("repo/chunk")) == "benchmarks/set/repo/chunk"


def test_benchmark_config_owns_server_and_path_settings(tmp_path: Path) -> None:
    config_path = tmp_path / "config.yml"
    config_path.write_text(
        "\n".join(
            [
                "local:",
                "  product_root: products",
                "  artifacts_root: artifacts",
                "environment:",
                "  GUTCH_S3_BUCKET_NAME: test-bucket",
                "dataset:",
                "  blender_versions:",
                "    - 5.2.0",
                "  blender_platforms:",
                "    - linux-x64",
                "container:",
                "  default_target: test-server",
                "  workspace_root: /tmp/benchmark",
                "  fixtures_root: /srv/fixtures",
                "  python: python3",
                "ssh:",
                "  test-server:",
                "    host: example.test",
                "    port: 2222",
                "    user: tester",
                "    identity_file: ~/.ssh/test_ed25519",
            ]
        ),
        encoding="utf-8",
    )

    loaded = load_benchmark_config(config_path)
    assert loaded.product_root == (tmp_path / "products").resolve()
    assert loaded.artifacts_root == (tmp_path / "artifacts").resolve()
    assert loaded.environment == {"GUTCH_S3_BUCKET_NAME": "test-bucket"}
    assert loaded.blender_versions == ("5.2.0",)
    assert loaded.blender_platforms == ("linux-x64",)


def test_container_package_contains_runner_inputs_without_external_binaries(tmp_path: Path) -> None:
    package_path = tmp_path / "benchmark-package.zip"
    container_runner._package_sources(package_path)

    with zipfile.ZipFile(package_path) as package:
        names = set(package.namelist())

    assert "tests/datasets/generate.py" in names
    assert "tests/datasets/prepare_blender.py" in names
    assert "tests/datasets/corpus/blender/case.json" in names
    assert "tests/benchmarks/docker/entry.py" in names
    assert all("Cargo.toml" not in name and "/bin/" not in name for name in names)


def test_container_runner_uploads_fixed_bundle_and_downloads_outputs(tmp_path: Path, monkeypatch) -> None:
    config_path = tmp_path / "config.yml"
    config_path.write_text(
        "\n".join(
            [
                "local:",
                f"  product_root: {tmp_path.as_posix()}/products",
                f"  artifacts_root: {tmp_path.as_posix()}/artifacts",
                "environment:",
                "  GUTCH_S3_BUCKET_NAME: test-bucket",
                "container:",
                "  default_target: test-server",
                "  workspace_root: /tmp/gutch-benchmark",
                "  fixtures_root: /tmp/fixtures",
                "  python: python3",
                "ssh:",
                "  test-server:",
                "    host: example.test",
                "    user: tester",
            ]
        ),
        encoding="utf-8",
    )

    class FakeLauncher:
        instances: list["FakeLauncher"] = []

        def __init__(self, target):
            self.target = target
            self.runs: list[list[str]] = []
            self.uploads: list[tuple[Path, str]] = []
            self.__class__.instances.append(self)

        def run(self, command, *, description, on_output=None):
            del description, on_output
            self.runs.append([str(value) for value in command])
            return CommandResult(tuple(str(value) for value in command), 0.0, 0, "", "")

        def upload(self, source, destination, *, description):
            del description
            self.uploads.append((source, destination))

        def download(self, source, destination, *, description):
            del source, description
            destination.parent.mkdir(parents=True, exist_ok=True)
            if destination.name == "container-result.json":
                destination.write_text(
                    json.dumps(
                        {
                            "fixture": "15_gutch_s3_cold_publication_cycle",
                            "dataset_path": "/home/tester/benchmark-fixtures/gutch-container-blender-cache",
                            "results_path": "/tmp/results.json",
                            "trace_path": "/tmp/trace.json",
                            "evidence_path": "/tmp/evidence.zip",
                        }
                    ),
                    encoding="utf-8",
                )
            elif destination.name == "evidence.zip":
                with zipfile.ZipFile(destination, "w") as archive:
                    archive.writestr("lanes/test/stats/capture.json", "{}")
            elif destination.name == "results.json":
                destination.write_text("[]\n", encoding="utf-8")
            else:
                destination.write_text('{"traceEvents": []}\n', encoding="utf-8")

    opened_traces = []
    monkeypatch.setattr(container_runner, "load_benchmark_fixture_open_perfetto", lambda _: True)
    monkeypatch.setattr(container_runner, "open_trace_in_perfetto",
                        lambda path, _log: opened_traces.append(path) or True)
    monkeypatch.setattr(container_runner, "DockerLauncher", FakeLauncher)
    binary_path = tmp_path / "gutch-cli"
    binary_path.write_bytes(b"linux-binary")
    monkeypatch.setattr(
        container_runner.DockerTarget,
        "start",
        lambda *_args: DockerTarget("test-container", workspace="/tmp/gutch-benchmark", services=SimpleNamespace(close=lambda: None)),
    )

    result_path = container_runner.run_docker_benchmark(
        fixture="15_gutch_s3_cold_publication_cycle",
        config_path=config_path,
        transfer_concurrency=1,
        transfer_max_concurrency=1,
    )

    launcher = FakeLauncher.instances[-1]
    assert opened_traces == [result_path.parent / "trace.json"]
    assert result_path.is_file()
    assert len(launcher.uploads) == 3
    container_command = next(command for command in launcher.runs if "--fixture" in command)
    assert "--product" not in container_command
    assert container_command[container_command.index("--fixture") + 1] == "15_gutch_s3_cold_publication_cycle"
    assert "--binary" in container_command
    assert container_command[container_command.index("--transfer-concurrency") + 1] == "1"
    assert container_command[container_command.index("--transfer-max-concurrency") + 1] == "1"


def test_container_progress_output_is_human_readable(capsys) -> None:
    container_runner._print_container_output(
        'CONTAINER_PROGRESS {"stage":"step","detail":"container-default-gutch lane started"}\n',
        False,
    )

    assert "[container] step: container-default-gutch lane started" in capsys.readouterr().err


def test_docker_benchmark_closes_services_on_failure(monkeypatch):
    outcomes = []
    target = SimpleNamespace(close=lambda: outcomes.append("closed"))
    monkeypatch.setattr(container_runner.DockerTarget, "start", lambda *_: target)
    def fail(*args):
        raise RuntimeError("fixture failed")
    monkeypatch.setattr(container_runner, "_execute_benchmark", fail)
    import pytest
    with pytest.raises(RuntimeError, match="fixture failed"):
        container_runner.run_docker_benchmark(fixture="15_gutch_s3_cold_publication_cycle")
    assert outcomes == ["closed"]


def test_container_menu_handles_keyboard_cancellation(monkeypatch, capsys) -> None:
    def cancel(_config):
        raise KeyboardInterrupt

    monkeypatch.setattr(container_launcher, "_run_interactive", cancel)

    assert container_launcher.main([]) == 130
    assert "Container benchmark cancelled." in capsys.readouterr().err


def test_storage_delete_and_abort_multipart_cover_all_pages(monkeypatch) -> None:
    target = StorageTarget("test", "S3_TEST", "http://s3", "auto", "bucket", True)

    class FakeClient:
        def __init__(self) -> None:
            self.deleted: list[dict[str, object]] = []
            self.aborted: list[dict[str, object]] = []

        def list_objects_v2(self, **kwargs):
            if kwargs.get("ContinuationToken"):
                return {"Contents": [{"Key": "benchmarks/set/object-2.bin"}], "IsTruncated": False}
            return {
                "Contents": [{"Key": "benchmarks/set/object-1.bin"}],
                "IsTruncated": True,
                "NextContinuationToken": "page-2",
            }

        def delete_objects(self, **kwargs):
            self.deleted.append(kwargs)
            return {"Errors": []}

        def list_multipart_uploads(self, **kwargs):
            if kwargs.get("KeyMarker"):
                return {
                    "Uploads": [{"Key": "benchmarks/set/object-2.bin", "UploadId": "upload-2"}],
                    "IsTruncated": False,
                }
            return {
                "Uploads": [{"Key": "benchmarks/set/object-1.bin", "UploadId": "upload-1"}],
                "IsTruncated": True,
                "NextKeyMarker": "benchmarks/set/object-2.bin",
                "NextUploadIdMarker": "upload-2",
            }

        def abort_multipart_upload(self, **kwargs):
            self.aborted.append(kwargs)

    client = FakeClient()
    monkeypatch.setattr(storage_workflow, "make_client", lambda _settings: client)

    assert storage_workflow.run_delete(target, prefix="benchmarks/set") == 2
    assert storage_workflow.run_abort_multipart_uploads(target, prefix="benchmarks/set") == 2
    assert len(client.deleted) == 1
    assert len(client.deleted[0]["Delete"]["Objects"]) == 2
    assert [item["UploadId"] for item in client.aborted] == ["upload-1", "upload-2"]


def test_storage_destructive_actions_require_confirmation(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(storage_cli, "load_storage_scenario", lambda _path: SimpleNamespace(
        targets=[SimpleNamespace(name="main-prod", enabled=True)], prefix="test-owned"))
    monkeypatch.setattr(storage_cli, "load_benchmark_environment", lambda _path: None)
    try:
        storage_cli.run_command(
            SimpleNamespace(
                fixture="s3_object_storage_8mib",
                action="delete",
                target="main-prod",
                work_dir=tmp_path,
                output_dir=None,
                prefix=None,
                yes=False,
                config=None,
            )
        )
    except ValueError as error:
        assert "requires --yes" in str(error)
    else:
        raise AssertionError("destructive storage actions must require confirmation")


def test_gutch_recovery_sequence_has_no_second_verify(tmp_path: Path) -> None:
    product = tmp_path / "product"
    source = product / "1.0.0-windows-x86-public"
    source.mkdir(parents=True)
    (source / "payload.bin").write_bytes(b"payload")

    class FakeRunner:
        def __init__(self) -> None:
            self.calls: list[tuple[str, ...]] = []
            self.sources: dict[str, Path] = {}

        def run(self, command, *, description, cwd=None, env=None, expected_failure=False):
            rendered = tuple(str(item) for item in command)
            self.calls.append(rendered)
            if rendered[1:3] == ("repo", "init"):
                Path(rendered[rendered.index("--repo") + 1]).mkdir(parents=True, exist_ok=True)
            elif rendered[1:3] == ("snapshot", "capture"):
                source_path = Path(rendered[rendered.index("--source") + 1])
                stats_path = Path(rendered[rendered.index("--output") + 1])
                snapshot_id = "snapshot-1"
                self.sources[snapshot_id] = source_path
                stats_path.parent.mkdir(parents=True, exist_ok=True)
                stats_path.write_text(json.dumps({"snapshot_id": snapshot_id}), encoding="utf-8")
            elif rendered[1:3] == ("installation", "reconstruct"):
                snapshot_id = rendered[rendered.index("--snapshot") + 1]
                destination = Path(rendered[rendered.index("--destination") + 1])
                shutil.copytree(self.sources[snapshot_id], destination)
                stats_path = Path(rendered[rendered.index("--output") + 1])
                stats_path.write_text(
                    json.dumps({"verify_timings": {"total_seconds": 0.25}}),
                    encoding="utf-8",
                )
            elif rendered[1:3] == ("debug", "corrupt-installation"):
                Path(rendered[rendered.index("--output") + 1]).write_text("{}", encoding="utf-8")
            elif rendered[1:3] == ("installation", "repair"):
                Path(rendered[rendered.index("--output") + 1]).write_text("{}", encoding="utf-8")
            return CommandResult(rendered, 0.01, 0, "", "")

    fake = FakeRunner()
    layout = RunLayout(tmp_path / "run")
    lane = LaneSpec(
        label="default",
        tool="gutch",
        profile=FixtureCatalog(Path(__file__).resolve().parents[1] / "benchmarks/comparison/fixtures").resolve("07_gutch_default_trace_with_corrupt_repair").lanes[0].profile,
    )
    result = LocalFixtureRunner(GutchCli(Path("gutch-cli.exe")), fake, layout).run_lane(
        product,
        lane,
        recovery_enabled=True,
    )
    verbs = [(call[1], call[2]) for call in fake.calls if len(call) > 2]
    assert verbs == [
        ("repo", "init"),
        ("snapshot", "capture"),
        ("installation", "reconstruct"),
        ("debug", "corrupt-installation"),
        ("installation", "repair"),
    ]
    assert result.metadata["verify_invocations"] == 1


def test_gutch_reconstructs_latest_capture_first(tmp_path: Path) -> None:
    product = tmp_path / "product"
    source_names = ("1.0.0-windows-x86-public", "2.0.0-windows-x86-public")
    for name in source_names:
        source = product / name
        source.mkdir(parents=True)
        (source / "payload.bin").write_bytes(name.encode("utf-8"))

    class OrderedRunner:
        def __init__(self) -> None:
            self.reconstruct_order: list[str] = []

        def run(self, command, *, description, cwd=None, env=None, expected_failure=False):
            rendered = tuple(str(item) for item in command)
            if rendered[1:3] == ("repo", "init"):
                Path(rendered[rendered.index("--repo") + 1]).mkdir(parents=True, exist_ok=True)
            elif rendered[1:3] == ("snapshot", "capture"):
                source = Path(rendered[rendered.index("--source") + 1])
                stats = Path(rendered[rendered.index("--output") + 1])
                stats.write_text(
                    json.dumps({"snapshot_id": source.name}),
                    encoding="utf-8",
                )
            elif rendered[1:3] == ("installation", "reconstruct"):
                snapshot_id = rendered[rendered.index("--snapshot") + 1]
                destination = Path(rendered[rendered.index("--destination") + 1])
                self.reconstruct_order.append(snapshot_id)
                destination.mkdir(parents=True, exist_ok=True)
                (destination / "payload.bin").write_bytes(snapshot_id.encode("utf-8"))
                stats = Path(rendered[rendered.index("--output") + 1])
                stats.write_text(
                    json.dumps({"verify_timings": {"total_seconds": 0.1}}),
                    encoding="utf-8",
                )
            return CommandResult(rendered, 0.01, 0, "", "")

    runner = OrderedRunner()
    lane = LaneSpec(
        label="ordered",
        tool="gutch",
        profile=FixtureCatalog(Path(__file__).resolve().parents[1] / "benchmarks/comparison/fixtures").resolve("07_gutch_default_trace_with_corrupt_repair").lanes[0].profile,
    )
    layout = RunLayout(tmp_path / "run")
    result = LocalFixtureRunner(GutchCli(Path("gutch-cli.exe")), runner, layout).run_lane(
        product,
        lane,
    )

    assert list(result.metadata["snapshot_metrics"]) == list(source_names)
    for values in result.metadata["snapshot_metrics"].values():
        assert values["backup"] == values["restore"] == 0.01
    assert runner.reconstruct_order == [source_names[1], source_names[0]]
    assert result.metadata["verify_invocations"] == 2


def test_gutch_command_prints_each_completed_lane(monkeypatch, tmp_path: Path, capsys) -> None:
    product = tmp_path / "product"
    product.mkdir()
    fixture = FixtureCatalog(Path(__file__).resolve().parents[1] / "benchmarks/comparison/fixtures").resolve("02_gutch_storage_layout_baseline")
    lane = LaneResult("bundled-default", "gutch-individual", {"backup": 1.25}, metadata={"differences": []})
    monkeypatch.setattr(gutch_cli, "resolve_gutch_executable", lambda *_args: tmp_path / "gutch.exe")
    monkeypatch.setattr(gutch_cli.FixtureCatalog, "resolve", lambda _self, _name: fixture)
    monkeypatch.setattr(
        gutch_cli,
        "run_fixture",
        lambda *_args, on_lane_complete, on_run_complete, **_kwargs: (
            on_lane_complete(lane),
            on_run_complete([lane]),
            [lane],
        )[2],
    )

    result = gutch_cli.run_command(
        SimpleNamespace(
            fixture=fixture.name,
            product=product,
            gutch_exe=None,
            artifacts_root=tmp_path / "artifacts",
            keep_artifacts=False,
        )
    )

    assert result == 0
    output = capsys.readouterr().out
    assert "=== Benchmark lane: bundled-default (gutch-individual) ===" in output
    assert "backup: 1.25" in output
    assert "Fixture Benchmark: 02_gutch_storage_layout_baseline / product" in output
    assert "| Comparison" in output


def test_fixture_perfetto_opens_after_lane_results(tmp_path: Path, monkeypatch) -> None:
    product = tmp_path / "product"
    product.mkdir()
    trace = tmp_path / "lane-trace.json"
    trace.write_text(json.dumps({"traceEvents": [{"name": "capture", "ts": 1}]}), encoding="utf-8")
    fixture = FixtureSpec(
        name="perfetto-fixture",
        runner="gutch-fixture",
        description="",
        delete_artifacts=False,
        open_perfetto=True,
        profile_mode="detailed",
        lanes=(LaneSpec("lane", "gutch", FixtureCatalog(Path(__file__).resolve().parents[1] / "benchmarks/comparison/fixtures").resolve("07_gutch_default_trace_with_corrupt_repair").lanes[0].profile),),
        source_path=tmp_path / "fixture.yaml",
    )
    opened: list[Path] = []

    class FakeFixtureRunner:
        def __init__(self, *_args, **_kwargs):
            pass

        def run_lane(self, *_args, **_kwargs):
            return LaneResult("lane", "gutch-individual", {"backup": 1.0}, {"capture": [str(trace)]})

    monkeypatch.setattr(gutch_workflow, "GutchDiagnosticRunner", FakeFixtureRunner)
    monkeypatch.setattr(benchmark_runner, "open_trace_in_perfetto", lambda path, _log: opened.append(path) or True)

    callbacks: list[str] = []
    gutch_workflow.run_fixture(
        fixture,
        product,
        GutchCli(tmp_path / "gutch.exe"),
        tmp_path / "artifacts",
        keep_artifacts=True,
        on_lane_complete=lambda result: callbacks.append(result.label),
    )

    assert callbacks == ["lane"]
    assert opened == [next((tmp_path / "artifacts").glob("**/trace.json"))]


def test_run_fixture_waits_between_measured_lanes(tmp_path: Path, monkeypatch) -> None:
    product = tmp_path / "product"
    product.mkdir()
    profile = FixtureCatalog(Path(__file__).resolve().parents[1] / "benchmarks/comparison/fixtures").resolve("07_gutch_default_trace_with_corrupt_repair").lanes[0].profile
    fixture = FixtureSpec(
        name="lane-waits",
        runner="gutch-fixture",
        description="",
        delete_artifacts=False,
        open_perfetto=False,
        lanes=(
            LaneSpec("first", "gutch", profile),
            LaneSpec("second", "gutch", profile),
        ),
        source_path=tmp_path / "fixture.yaml",
        lane_wait_seconds=2.0,
        step_wait_seconds=0.0,
    )

    class FakeFixtureRunner:
        def __init__(self, *_args, **_kwargs):
            pass

        def run_lane(self, _product, lane, _recovery_enabled, **_kwargs):
            return LaneResult(lane.label, lane.kind, {"backup": 1.0})

    waits: list[float] = []
    monkeypatch.setattr(gutch_workflow, "GutchDiagnosticRunner", FakeFixtureRunner)
    monkeypatch.setattr(benchmark_runner.time, "sleep", waits.append)

    gutch_workflow.run_fixture(
        fixture,
        product,
        GutchCli(tmp_path / "gutch.exe"),
        tmp_path / "artifacts",
        keep_artifacts=True,
    )

    assert waits == [2.0]


def test_gutch_profile_table_includes_hash_details(tmp_path: Path, capsys) -> None:
    trace = tmp_path / "trace.json"
    trace.write_text(
        json.dumps(
            {
                "traceEvents": [
                    {
                        "name": "installation.file.verify",
                        "ph": "B",
                        "ts": 0,
                        "pid": 1,
                        "tid": 1,
                        "args": {"size_bytes": 4194304, "hash_mode": "full", "configured_workers": 2},
                    },
                    {"name": "installation.file.verify", "ph": "E", "ts": 1_000_000, "pid": 1, "tid": 1},
                ]
            }
        ),
        encoding="utf-8",
    )

    print_gutch_profile_table([LaneResult("lane", "gutch-individual", traces={"reconstruct": [str(trace)]})])

    output = capsys.readouterr().out
    assert "Gutch Perfetto Profiles" in output
    assert "Hash files" in output
    assert "Reconstruct workers" in output


def test_combined_trace_preserves_worker_activity_and_gaps(tmp_path: Path) -> None:
    events = [
        {"name": "thread_name", "ph": "M", "pid": 1, "tid": 2, "ts": 0,
         "args": {"name": "worker-0"}},
        {"name": "cli.command", "ph": "X", "pid": 1, "tid": 1, "ts": 0, "dur": 100},
        {"name": "capture.scan.file", "ph": "X", "pid": 1, "tid": 2, "ts": 20, "dur": 5},
        {"name": "capture.scan.file", "ph": "X", "pid": 1, "tid": 2, "ts": 40, "dur": 5},
    ]
    first, second, output = (tmp_path / name for name in ('first.json', 'second.json', 'trace.json'))
    for source in (first, second):
        source.write_text(json.dumps({"traceEvents": events}))
    combine_labeled_traces([('lane', first), ('lane', second)], output)
    combined = json.loads(output.read_text())["traceEvents"]
    workers = [event for event in combined if event['name'] == 'thread_name']
    assert len(workers) == 1
    assert len({event['tid'] for event in workers}) == 1
    spans = [event for event in combined if event['name'] == 'capture.scan.file']
    assert [(event['ts'], event['dur']) for event in spans] == [(20, 5), (40, 5), (121, 5), (141, 5)]
    assert sum(event.get('ph') == 'X' for event in combined) == 6
    assert not output.with_name('trace.detailed.json').exists()


def test_successful_cleanup_retains_trace(tmp_path: Path) -> None:
    layout = RunLayout(tmp_path / 'run')
    lane = layout.lane('worker')
    (lane.payload / 'data').write_bytes(b'payload')
    for name in ('trace.json',):
        (layout.root / name).write_text('{"traceEvents": []}')
    layout.finish()
    assert not lane.root.exists()
    for name in ('trace.json',):
        assert json.loads((layout.root / name).read_text()) == {"traceEvents": []}


def test_perfetto_url_changes_when_trace_contents_change(tmp_path: Path, monkeypatch) -> None:
    import urllib.parse
    import comparison_benchmark_suite.support.traces as traces

    class Server:
        def __init__(self, *_args):
            self.trace_requested = False

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            pass

        def handle_request(self):
            self.trace_requested = True

    opened = []
    monkeypatch.setattr(traces, '_TraceServer', Server)
    monkeypatch.setattr(traces.webbrowser, 'open_new_tab', lambda url: opened.append(url) or True)
    path = tmp_path / 'trace with spaces.json'
    for name in ("first", "new"):
        path.write_text(json.dumps({"traceEvents": [
            {"name": name, "ph": "X", "pid": 1, "tid": 1, "ts": 0, "dur": 1},
        ]}))
        assert traces.open_trace_in_perfetto(path, lambda _: None)
    urls = [urllib.parse.parse_qs(urllib.parse.urlsplit(url).fragment.split('?', 1)[1])['url'][0]
            for url in opened]
    assert urls[0] != urls[1]
    for url in urls:
        parsed = urllib.parse.urlsplit(url)
        assert urllib.parse.unquote(parsed.path) == '/trace with spaces.json'
        assert len(urllib.parse.parse_qs(parsed.query)['v'][0]) == 12


def test_perfetto_does_not_open_empty_or_metadata_only_traces(tmp_path: Path, monkeypatch) -> None:
    import comparison_benchmark_suite.support.traces as traces

    def unexpected_browser_open(_url):
        raise AssertionError("Empty traces must not open a browser")

    monkeypatch.setattr(traces.webbrowser, "open_new_tab", unexpected_browser_open)
    path = tmp_path / "trace.json"
    for events in ([], [{"ph": "M", "name": "process_name", "args": {"name": "empty"}}]):
        path.write_text(json.dumps({"traceEvents": events}))
        messages = []
        assert not traces.open_trace_in_perfetto(path, messages.append)
        assert "no recorded events" in messages[-1]


def test_jc_fixture_selects_chunker_sizes_with_matching_packaging():
    fixture = load_fixture(Path("tests/benchmarks/comparison/fixtures/16_gutch_fastcdc_vs_jc.yaml"))
    assert [lane.profile.chunker for lane in fixture.lanes] == ["fastcdc", "jc"]
    args = [GutchCli.profile_args(lane.profile) for lane in fixture.lanes]
    assert args[0][:2] == ["--chunker", "fastcdc"]
    assert args[1][:2] == ["--chunker", "jc"]
    assert args[0][2:4] == ["--chunk-preset", "128KiB"]
    assert args[1][2:4] == ["--chunk-preset", "256KiB"]
    assert args[0][4:] == args[1][4:]


def test_vram_fixture_exposes_three_chunkers():
    fixture = load_fixture(Path("tests/benchmarks/comparison/fixtures/17_gutch_fastcdc_jc_vram256.yaml"))
    assert [lane.profile.chunker for lane in fixture.lanes] == ["fastcdc", "jc", "vram256"]
    args = GutchCli.profile_args(fixture.lanes[2].profile)
    assert args[:4] == ["--chunker", "vram256", "--chunk-preset", "128KiB"]
