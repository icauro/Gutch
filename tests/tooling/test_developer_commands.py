"""Developer commands select real cases and preserve useful failure evidence."""
import json
from pathlib import Path

import pytest

from tools import runner
from tests.run import SCENARIOS, selected_scenarios


def test_repository_suite_uses_discoverable_case_paths():
    cases = selected_scenarios("repository", None)
    assert cases
    assert all(case.name.startswith("repository/") for case in cases)
    assert all((runner.ROOT / "tests/scenarios" / (case.name + ".py")).is_file() for case in SCENARIOS)
    assert not any(case.suite == "s3" for case in selected_scenarios("check", None))
    assert any(case.suite == "s3" for case in selected_scenarios("all", None))


def test_benchmark_without_arguments_lists_without_starting_a_workload(monkeypatch):
    calls = []
    monkeypatch.setattr(runner, "run_module", lambda *args: calls.append(args) or 0)
    assert runner.main(["benchmark"]) == 0
    assert calls == [("tests.benchmarks.comparison", ["list"])]


@pytest.mark.parametrize("arguments,module,forwarded", [
    (["benchmark", "storage", "--help"], "tests.benchmarks.s3_throughput", ["--help"]),
    (["benchmark", "interactive"], "tests.benchmarks.comparison", []),
])
def test_specialized_benchmarks_share_the_developer_entrypoint(monkeypatch, arguments, module, forwarded):
    calls = []
    monkeypatch.setattr(runner, "run_module", lambda *args: calls.append(args) or 0)
    assert runner.main(arguments) == 0
    assert calls == [(module, forwarded)]


def test_replay_validates_case_and_reports_source_changes(tmp_path, monkeypatch, capsys):
    from tests.support import reproduction
    monkeypatch.setattr(reproduction, "source_revision", lambda: "current")
    case = SCENARIOS[0].name
    record = {"schema_version": 1, "case": case, "revision": "original"}
    path = tmp_path / "replay.json"
    path.write_text(json.dumps(record))
    calls = []
    monkeypatch.setattr(runner, "run_module", lambda *args: calls.append(args) or 0)
    assert runner.replay(tmp_path) == 0
    assert "Source changed" in capsys.readouterr().out
    assert calls == [("tests.run", ["--case", case, "--keep-artifacts"])]
    assert json.loads(path.read_text()) == record
    record["case"] = "../../arbitrary_module"
    path.write_text(json.dumps(record))
    with pytest.raises(ValueError, match="Unsupported replay"):
        runner.replay(tmp_path)
    assert len(calls) == 1


def test_clean_keeps_failed_payloads_and_successful_reports(tmp_path, monkeypatch):
    monkeypatch.setattr(runner, "ROOT", tmp_path)
    for status in ("passed", "failed", "running"):
        root = tmp_path / "tests/.artifacts/runs/gutch" / status
        payload = root / "cases/repository/example/payload"
        payload.mkdir(parents=True)
        (payload / "source.bin").write_bytes(b"evidence")
        (root / "run.json").write_text(json.dumps({"status": status, "cases": [{"evidence": "cases/repository/example"}]}))
    assert runner.clean() == 0
    for status in ("passed", "failed", "running"):
        root = tmp_path / "tests/.artifacts/runs/gutch" / status
        assert (root / "run.json").is_file()
        assert (root / "cases/repository/example/payload").exists() == (status != "passed")


def test_clean_rejects_a_case_outside_its_run(tmp_path, monkeypatch):
    monkeypatch.setattr(runner, "ROOT", tmp_path)
    root = tmp_path / "tests/.artifacts/runs/gutch/example"
    root.mkdir(parents=True)
    (root / "run.json").write_text(json.dumps({"status": "passed", "cases": [{"evidence": "../../../../.."}]}))
    with pytest.raises(ValueError, match="escapes"):
        runner.clean()


def test_source_revision_changes_when_test_or_engine_bytes_change(tmp_path, monkeypatch):
    from tools import build_image
    from tests.support.reproduction import source_revision
    source = tmp_path / "code.rs"
    source.write_text("first")
    monkeypatch.setattr(build_image, "source_files", lambda: [(source, Path("src/code.rs"))])
    original = source_revision()
    assert original == source_revision()
    source.write_text("second")
    assert original != source_revision()
