"""Developer commands select real cases and preserve useful failure evidence."""

import json
from pathlib import Path

import pytest

from _tools import runner
from tests.run import SCENARIOS, selected_scenarios


def test_repository_suite_uses_discoverable_case_paths():
    cases = selected_scenarios("repository", None)
    assert cases
    assert all(case.name.startswith("repository/") for case in cases)
    assert all(
        (runner.ROOT / "tests/scenarios" / (case.name + ".py")).is_file()
        for case in SCENARIOS
    )
    assert not any(case.suite == "s3" for case in selected_scenarios("check", None))
    assert any(case.suite == "s3" for case in selected_scenarios("all", None))


def test_benchmark_without_arguments_lists_internal_fixtures(monkeypatch):
    calls = []
    monkeypatch.setattr(runner, "run_module", lambda *args: calls.append(args) or 0)
    assert runner.main(["benchmark"]) == 0
    assert calls == [("tests.benchmarks.comparison", ["list"])]


@pytest.mark.parametrize(
    "arguments,module,forwarded",
    [
        (
            ["benchmark", "storage", "--help"],
            "tests.benchmarks.s3_throughput",
            ["--help"],
        ),
        (["benchmark", "interactive"], "tests.benchmarks.comparison", []),
    ],
)
def test_specialized_benchmarks_share_the_developer_entrypoint(
    monkeypatch, arguments, module, forwarded
):
    calls = []
    monkeypatch.setattr(runner, "run_module", lambda *args: calls.append(args) or 0)
    assert runner.main(arguments) == 0
    assert calls == [(module, forwarded)]


def test_replay_validates_case_and_reports_source_changes(
    tmp_path, monkeypatch, capsys
):
    from tests.helpers import reproduction

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
        root = tmp_path / ".artifacts/tests/runs/gutch" / status
        payload = root / "cases/repository/example/payload"
        payload.mkdir(parents=True)
        (payload / "source.bin").write_bytes(b"evidence")
        (root / "run.json").write_text(
            json.dumps(
                {
                    "status": status,
                    "cases": [
                        {
                            "name": "repository/example",
                            "status": status,
                            "evidence": "cases/repository/example",
                        }
                    ],
                }
            )
        )
    assert runner.clean() == 0
    for status in ("passed", "failed", "running"):
        root = tmp_path / ".artifacts/tests/runs/gutch" / status
        assert (root / "run.json").is_file()
        assert (root / "cases/repository/example/payload").exists() == (
            status != "passed"
        )


def test_clean_rejects_a_case_outside_its_run(tmp_path, monkeypatch):
    monkeypatch.setattr(runner, "ROOT", tmp_path)
    root = tmp_path / ".artifacts/tests/runs/gutch/example"
    root.mkdir(parents=True)
    (root / "run.json").write_text(
        json.dumps({"status": "passed", "cases": [{"evidence": "../../../../.."}]})
    )
    with pytest.raises(ValueError, match="escapes"):
        runner.clean()


def test_source_revision_changes_when_test_or_engine_bytes_change(
    tmp_path, monkeypatch
):
    from _tools import build_image
    from tests.helpers.reproduction import source_revision

    source = tmp_path / "code.rs"
    source.write_text("first")
    monkeypatch.setattr(
        build_image, "source_files", lambda: [(source, Path("src/code.rs"))]
    )
    original = source_revision()
    assert original == source_revision()
    source.write_text("second")
    assert original != source_revision()


def test_replay_and_minimize_recreate_a_corpus_dataset_instead_of_its_prepared_paths(
    tmp_path,
):
    from tests.helpers.minimize import trial_arguments
    from tests.helpers.reproduction import lifecycle_arguments

    options = {
        "seed": 7,
        "sequence": "capture,delete",
        "dataset": "03_duplicates",
        "repository_version": "v1",
        "repo": "prepared/source",
        "expected": "prepared/expected.json",
    }
    arguments = lifecycle_arguments(options)
    assert arguments == [
        "--seed", "7", "--sequence", "capture,delete",
        "--dataset", "03_duplicates", "--repository-version", "v1",
    ]
    trial = trial_arguments(options, ["delete"], tmp_path / "trial-1")
    assert trial[trial.index("--sequence") + 1] == "delete"
    assert trial.count("--sequence") == 1
    assert "--dataset" in trial and "--repo" not in trial
    assert lifecycle_arguments({"repo": "r", "expected": "e"}) == [
        "--repo", "r", "--expected", "e",
    ]


def test_minimize_finds_each_trial_report_under_its_own_root(tmp_path):
    from tests.helpers.minimize import trial_report

    assert trial_report(tmp_path) is None
    report = tmp_path / "runs/gutch/20260101T000000-abcdef/cases/repository/lifecycle/results.json"
    report.parent.mkdir(parents=True)
    report.write_text("{}", encoding="utf-8")
    assert trial_report(tmp_path) == report


def test_repository_version_requires_a_dataset(capsys):
    from tests.run import main

    with pytest.raises(SystemExit):
        main(["--repository-version", "v2", "--list"])
    assert "--repository-version requires --dataset" in capsys.readouterr().err


def test_fuzz_command_parses_targets_and_seeds_decoders_with_bundles(
    tmp_path, monkeypatch
):
    from tests.helpers import fuzz

    seeds = list(fuzz.seed_inputs())
    assert any(path.suffix == ".gtcbin" for path in seeds)
    assert any(path.parent.parent.name == "bundles" for path in seeds)
    assert all(path.stat().st_size <= fuzz.MAX_INPUT_BYTES for path in seeds)
    engine = (runner.ROOT / "src/tests/engine_fuzz.rs").read_text(encoding="utf-8")
    assert f"bytes.len() > {fuzz.MAX_INPUT_BYTES}" in engine

    calls = []
    monkeypatch.setattr(fuzz.shutil, "which", lambda name: name)
    monkeypatch.setattr(fuzz, "REPOSITORY", tmp_path)
    monkeypatch.setattr(fuzz, "execute", lambda command, *args, **kwargs: calls.append(command))
    assert fuzz.main(["--target", "bundle_roundtrip", "--seconds", "1", "--seed", "3"]) == 0
    assert len(calls) == 1
    assert "engine_fuzz::fuzz_bundle_roundtrip" in calls[0]
    assert calls[0][calls[0].index("-S") + 1] == "3"
    assert not any((tmp_path / ".artifacts/tests/fuzz/bundle_roundtrip/corpus").iterdir())


def test_fuzz_reports_a_missing_cargo_bolero(monkeypatch):
    from tests.helpers import fuzz

    monkeypatch.setattr(fuzz.shutil, "which", lambda name: None)
    with pytest.raises(SystemExit, match="cargo install cargo-bolero"):
        fuzz.main(["--seconds", "1"])
