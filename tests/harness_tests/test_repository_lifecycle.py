"""Guard input independence, immutable originals and artifact ownership."""
import ast
import json
from pathlib import Path
import subprocess
import sys

import pytest

from tests.datasets.trees import generated
from tests.repository_suite import copy_repository
from tests.helpers.filesystem import inventory
from tools.artifacts import compact_success, prune_successful_runs


def test_generator_is_deterministic_and_consumers_can_validate_independently(tmp_path):
    first = generated(tmp_path / "first", 123, 2)
    second = generated(tmp_path / "second", 123, 2)
    changed = generated(tmp_path / "changed", 123, 3)
    assert first.expected == second.expected == inventory(first.root) == inventory(second.root)
    assert changed.expected != first.expected
    assert changed.expected["shared.bin"] == first.expected["shared.bin"]
    assert inventory(changed.root) == changed.expected


def test_dataset_modules_do_not_import_consumers():
    root = Path(__file__).resolve().parents[1] / "datasets"
    for path in root.rglob("*.py"):
        for node in ast.walk(ast.parse(path.read_text(encoding="utf-8"))):
            names = [node.module or ""] if isinstance(node, ast.ImportFrom) else (
                [alias.name for alias in node.names] if isinstance(node, ast.Import) else [])
            assert not any(name.startswith(("tests.helpers", "tests.benchmarks", "tests.scenarios", "tools.")) for name in names), path


def test_copy_repository_leaves_original_unchanged_and_rejects_nested_output(tmp_path):
    source = tmp_path / "original"
    source.mkdir()
    (source / "repo_metadata.json").write_text("{}")
    (source / "data.bin").write_bytes(b"original")
    before = inventory(source)
    destination = tmp_path / "copy"
    copy_repository(source, destination)
    (destination / "data.bin").write_bytes(b"changed")
    assert inventory(source) == before
    with pytest.raises(ValueError, match="must not contain"):
        copy_repository(source, source / "nested")
    assert inventory(source) == before


def test_compaction_removes_inputs_caches_and_logs_but_keeps_summary(tmp_path):
    (tmp_path / "run.json").write_text("{}")
    (tmp_path / "summary.md").write_text("passed")
    for name in ("cases/input", "tooling-temp/cache", "tooling-tests.log"):
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("temporary")
    compact_success(tmp_path, keep=True)
    assert (tmp_path / "cases/input").exists()
    compact_success(tmp_path)
    assert {p.name for p in tmp_path.iterdir()} == {"run.json", "summary.md"}


def test_retention_never_prunes_failed_active_or_pinned_runs(tmp_path):
    for index in range(5):
        root = tmp_path / str(index)
        root.mkdir()
        (root / "run.json").write_text(json.dumps({"status": "passed", "cleanup_complete": True}))
    for status in ("failed", "running", "pinned", "finalizing"):
        root = tmp_path / status
        root.mkdir()
        (root / "run.json").write_text(json.dumps({"status": "passed" if status in ("pinned", "finalizing") else status,
             "keep_artifacts": status == "pinned", "cleanup_complete": status != "finalizing"}))
    assert len(prune_successful_runs(tmp_path, retain=2)) == 3
    assert all((tmp_path / name).exists() for name in ("failed", "running", "pinned", "finalizing"))


def test_benchmark_compaction_removes_owned_external_caches(tmp_path):
    from tests.benchmarks.support.artifacts import RunLayout
    from tests.benchmarks.comparison.external import _plakar_cache_directory
    layout = RunLayout(tmp_path)
    lane = layout.lane("comparison")
    cache = _plakar_cache_directory(lane.root)
    cache.mkdir()
    (cache / "content").write_bytes(b"cached")
    (lane.payload / "content").write_bytes(b"payload")
    (tmp_path / "results.json").write_text("[]")
    layout.finish()
    assert not cache.exists() and not lane.root.exists()
    assert (tmp_path / "results.json").exists()
    assert json.loads((tmp_path / "run.json").read_text())["cleanup_complete"]


def test_clean_migrates_old_success_without_dangling_evidence_links(tmp_path, monkeypatch):
    from tools import runner
    monkeypatch.setattr(runner, "ROOT", tmp_path)
    run = tmp_path / "tests/.artifacts/runs/gutch/old"
    evidence = run / "cases/example"
    evidence.mkdir(parents=True)
    (evidence / "input.bin").write_bytes(b"temporary")
    (run / "run.json").write_text(json.dumps({"status": "passed", "cases": [
        {"name": "example", "status": "passed", "evidence": "cases/example"}]}))
    (run / "summary.md").write_text("[evidence](cases/example/results.json)")
    assert runner.clean() == 0
    assert not evidence.exists()
    assert "cases/example" not in (run / "summary.md").read_text()
    assert json.loads((run / "run.json").read_text())["cleanup_complete"]


@pytest.mark.parametrize("module", ["tests.helpers.filesystem", "tests.benchmarks.support.filesystem"])
def test_unreadable_directories_cannot_pass_inventory_validation(tmp_path, monkeypatch, module):
    import importlib
    import os
    scanner = importlib.import_module(module)
    restricted = tmp_path / "restricted"
    restricted.mkdir()
    (restricted / "lost.bin").write_bytes(b"important")
    scandir = os.scandir
    def denied(path):
        if Path(path) == restricted:
            raise PermissionError("unreadable source")
        return scandir(path)
    monkeypatch.setattr(os, "scandir", denied)
    with pytest.raises(PermissionError, match="unreadable"):
        scanner.inventory(tmp_path)


def test_benchmark_deadline_terminates_hung_command_and_records_failure(tmp_path):
    from tests.benchmarks.support.command import CommandRunner
    runner = CommandRunner(timeout_seconds=0.2, progress=False)
    runner.evidence_directory = tmp_path
    with pytest.raises(subprocess.TimeoutExpired):
        runner.run([sys.executable, "-c", "import time; time.sleep(60)"], description="hung command")
    record = json.loads((tmp_path / "command-failure.json").read_text())
    assert record["timeout_seconds"] == 0.2


def test_s3_cleanup_is_scoped_and_reports_delete_failures():
    from tests.benchmarks.support.s3 import delete_published_prefix, S3Settings
    settings = S3Settings("bucket", "", "auto", "", "")
    prefix = "runs/" + "a" * 32
    class Client:
        def list_objects_v2(self, **kwargs):
            assert kwargs["Prefix"] == prefix + "/"
            return {"Contents": [{"Key": prefix + "/artifact"}]}
        def delete_objects(self, **kwargs):
            assert kwargs["Delete"]["Objects"] == [{"Key": prefix + "/artifact"}]
            return {"Errors": [{"Code": "AccessDenied"}]}
    with pytest.raises(ValueError, match="UUID"):
        delete_published_prefix(Client(), settings, "")
    with pytest.raises(RuntimeError, match="AccessDenied"):
        delete_published_prefix(Client(), settings, prefix)
