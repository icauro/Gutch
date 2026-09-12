"""Operations: Capture, change constraints, verify, restore and fetch snapshots.

Expected: Invalid names reject operations without modifying existing payloads;
clearing constraints and fetching a compatible constrained repository succeed.
"""
import json
import os
from tests.helpers.filesystem import inventory, repository_inventory
from tests.helpers.delivery import DeliveryServer, fetch_arguments


def run(case):
    source = case.payload / "source"
    source.mkdir()
    # Valid on both hosts; the Windows policy rejects case-sensitive directory aliases.
    # On Windows use a name within NTFS's UTF-16 budget but beyond the Linux byte budget.
    name = "é" * 128 if os.name == "nt" else "report:final.txt"
    target = "linux-ext4" if os.name == "nt" else "windows-ntfs"
    (source / name).write_bytes(b"content")
    repo = case.init()
    snapshot = case.capture(repo, source)["snapshot_id"]
    error_path = case.root / "constraint-error.json"
    before = repository_inventory(repo)
    case.command("repo", "constraints", "--repo", repo, "--target", target,
                 "--error-json-out", error_path, success=False)
    assert repository_inventory(repo) == before
    failure = json.loads(error_path.read_text(encoding="utf-8"))
    assert failure["code"] == "filename_constraints"
    assert failure["details"]["operation"] == "change constraints"
    assert failure["details"]["violations"][0]["path"] == name
    assert failure["details"]["violations"][0]["target"] == target

    constrained = case.init("constrained")
    case.command("repo", "constraints", "--repo", constrained, "--target", target)
    before = repository_inventory(constrained)
    case.command("snapshot", "capture", "--repo", constrained, "--source-dir", source,
                 "--yes", "--error-json-out", error_path, success=False)
    assert repository_inventory(constrained) == before
    assert json.loads(error_path.read_text(encoding="utf-8"))["details"]["operation"] == "capture"

    # External metadata editing cannot bypass verification or restoration checks.
    metadata_path = repo / "repo_metadata.json"
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    metadata["constraints"] = {"targets": [target]}
    metadata_path.write_text(json.dumps(metadata), encoding="utf-8")
    with DeliveryServer(repo) as server:
        cache = case.payload / "rejected-cache"
        case.command(*fetch_arguments(case, cache, snapshot, repo, server.url),
                     "--error-json-out", error_path, success=False)
        assert json.loads(error_path.read_text(encoding="utf-8"))["details"]["operation"] == "fetch"
        assert not (cache / "repo_metadata.json").exists()
        assert not list((cache / "snapshots").glob("*/snapshot_metadata.json"))
    case.command("repo", "verify", "--repo", repo, success=False)
    destination = case.payload / "destination"
    destination.mkdir()
    (destination / "keep.txt").write_text("keep", encoding="utf-8")
    before = inventory(destination)
    case.command("snapshot", "reconstruct", "--repo", repo, "--snapshot-id", snapshot,
                 "--destination", destination, success=False)
    assert inventory(destination) == before
    case.command("repo", "constraints", "--repo", repo, "--clear")
    case.command("repo", "verify", "--repo", repo)
    assert "constraints" not in json.loads(metadata_path.read_text(encoding="utf-8"))

    # A new cache must accept a compatible constrained repository without a
    # false conflict against the cache's initially empty metadata.
    safe_source = case.payload / "safe-source"
    safe_source.mkdir()
    (safe_source / "safe.txt").write_bytes(b"safe")
    safe_snapshot = case.capture(constrained, safe_source)["snapshot_id"]
    with DeliveryServer(constrained) as server:
        cache = case.payload / "accepted-cache"
        case.command(*fetch_arguments(case, cache, safe_snapshot, constrained, server.url))
        assert json.loads((cache / "repo_metadata.json").read_text(encoding="utf-8"))["constraints"]["targets"] == [target]
