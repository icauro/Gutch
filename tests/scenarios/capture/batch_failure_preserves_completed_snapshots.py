"""Operations: Batch-capture identical sources with conflicting tags into an empty repository.

Expected: The second capture fails; the first snapshot keeps its tags and reconstructs exactly.
"""
import json
from tests.datasets.generate import basic
from tests.helpers.filesystem import write_json


def run(case):
    repo = case.init()
    sources = case.payload / "sources"
    first = basic(sources / "first")
    basic(sources / "second")
    write_json(sources / ".gutch_source_config.json", {
        "ingestion_mode": "explicit", "explicit_ingestion_rules": {"snapshots": [
            {"path": "first", "tags": ["version::1.0.0"]},
            {"path": "second", "tags": ["version::2.0.0"]},
        ]}})
    case.command("snapshot", "capture", "--source-root", sources, "--repo", repo, "--yes", success=False)
    snapshots = list((repo / "snapshots").glob("*/snapshot_metadata.json"))
    assert len(snapshots) == 1
    assert json.loads(snapshots[0].read_text())["snapshot_tags"] == ["version::1.0.0"]
    destination = case.payload / "completed-snapshot"
    case.reconstruct(repo, snapshots[0].parent.name, destination)
    case.compare(first, destination)
