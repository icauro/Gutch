"""Operations: Capture a nested explicit source, then add an undeclared sibling and recapture.

Expected: Only the declared source is captured, its tags match, and its output is exact.
"""
import json
from tests.support.datasets import basic


def run(case):
    repo = case.init()
    source = case.payload / "sources"
    data = basic(source / "nested" / "arbitrary-name")
    config = source / ".gutch_source_config.json"
    config.write_text(json.dumps({"ingestion_mode": "explicit", "explicit_ingestion_rules": {
        "snapshots": [{"path": "nested/arbitrary-name", "tags": ["os::windows", "arch::x86_64"]}]}}))
    case.command("snapshot", "capture", "--source-root", source, "--repo", repo, "--yes")
    snapshots = list((repo / "snapshots").glob("*/snapshot_metadata.json"))
    assert len(snapshots) == 1
    destination = case.payload / "installed"
    case.reconstruct(repo, snapshots[0].parent.name, destination)
    case.compare(data, destination)
    assert json.loads(snapshots[0].read_text())["snapshot_tags"] == ["arch::x86_64", "os::windows"]

    # Explicit discovery excludes undeclared siblings and accepts ./ paths.
    basic(source / "undeclared", 2)
    config.write_text(json.dumps({"ingestion_mode": "explicit", "explicit_ingestion_rules": {
        "snapshots": [{"path": "./nested/arbitrary-name", "tags": ["os::windows", "arch::x86_64"]}]}}))
    case.command("snapshot", "capture", "--source-root", source, "--repo", repo, "--yes")
    assert len(list((repo / "snapshots").iterdir())) == 1
