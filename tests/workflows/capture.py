"""Capture identity, deduplication, profiles, and source ingestion scenarios."""
import json

from ..support.datasets import basic, boundaries


def capture_roundtrip(case):
    data = basic(case.payload / "source")
    repo = case.init()
    first = case.capture(repo, data.root)
    again = case.capture(repo, data.root)
    assert first["snapshot_id"] == again["snapshot_id"]
    assert again["generated_bundle_count"] == 0
    destination = case.payload / "installed"
    case.reconstruct(repo, first["snapshot_id"], destination)
    case.compare(data, destination)
    assert case.verify(repo, first["snapshot_id"], destination)["verification_passed"]


def capture_deduplication(case):
    repo = case.init()
    first = case.capture(repo, basic(case.payload / "v1").root)
    data = basic(case.payload / "v2", 2)
    second = case.capture(repo, data.root)
    assert first["snapshot_id"] != second["snapshot_id"]
    assert second["reused_chunk_count"] > 0
    for label, stats, version in [("v1", first, 1), ("v2", second, 2)]:
        destination = case.payload / f"install-{label}"
        case.reconstruct(repo, stats["snapshot_id"], destination)
        case.compare(basic(case.payload / f"expected-{label}", version), destination, label)


def capture_profiles(case):
    data = boundaries(case.payload / "source")
    for compression, storage in [("none", "bundled"), ("lz4", "bundled"), ("zstd", "loose_chunks")]:
        label = f"{compression}-{storage}"
        repo = case.init(label)
        captured = case.capture(repo, data.root, "--compression-algorithm", compression, "--storage-mode", storage)
        destination = case.payload / f"out-{label}"
        case.reconstruct(repo, captured["snapshot_id"], destination)
        case.compare(data, destination, label)


def capture_metadata_conflict(case):
    data = basic(case.payload / "source")
    repo = case.init()
    captured = case.capture(repo, data.root, "--tag", "version::1.0.0")
    path = repo / "snapshots" / captured["snapshot_id"] / "snapshot_metadata.json"
    before = path.read_bytes()
    case.command("snapshot", "capture", "--repo", repo, "--source-dir", data.root,
                 "--tag", "version::2.0.0", "--yes", success=False)
    assert path.read_bytes() == before


def source_ingestion(case):
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


def source_ingestion_auto(case):
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
