"""Operations: Capture without a source config, configure an ignored folder, and capture again.

Expected: A config is created, ignored content is excluded, and tags are not inferred from
names.
"""
import json
from ..support.datasets import basic


def run(case):
    source = case.payload / "sources"
    data = basic(source / "v1.0.0-windows-x86_64-2026_07_14")
    repo = case.init()
    case.command("snapshot", "capture", "--source-root", source, "--repo", repo, "--yes")
    config = source / ".gutch_source_config.json"
    assert config.is_file(), "first capture must create the source configuration"
    basic(source / ".archives", 2)
    config.write_text(json.dumps({"ingestion_mode": "auto", "auto_ingestion_rules": {"ignore_paths": [".archives"]}}))
    case.command("snapshot", "capture", "--source-root", source, "--repo", repo, "--yes")
    snapshots = list((repo / "snapshots").glob("*/snapshot_metadata.json"))
    assert len(snapshots) == 1
    tags = json.loads(snapshots[0].read_text())["snapshot_tags"]
    # Auto discovery without configured tag rules does not infer tags from names.
    assert tags == []
    destination = case.payload / "installed"
    case.reconstruct(repo, snapshots[0].parent.name, destination)
    case.compare(data, destination)
