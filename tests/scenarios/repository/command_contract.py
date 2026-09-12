"""Operations: Exercise relative paths, output formats, dry-run isolation and preset validation.

Expected: Previews preserve source/repository/installation content; retired syntax fails.
"""
import json
import os

from tests.helpers.cli import REPOSITORY
from tests.helpers.filesystem import repository_inventory


def run(case):
    repo = case.init()
    source = case.payload / "release"
    source.mkdir()
    (source / "empty").touch()
    (source / "readme").write_bytes(b"release")
    before = repository_inventory(repo)
    relative_repo = os.path.relpath(repo, REPOSITORY)
    relative_source = os.path.relpath(source, REPOSITORY)
    plans = case.stats("snapshot", "capture", "--repo", relative_repo,
                       "--source", relative_source, "--dry-run")
    assert len(plans) == 1
    assert repository_inventory(repo) == before
    assert sorted(p.name for p in source.iterdir()) == ["empty", "readme"]
    case.command("snapshot", "capture", "--repo", repo, "--source", source,
                 "--chunk-preset", "4MiB", "--bundle-preset", "4MiB", "--yes", success=False)
    assert repository_inventory(repo) == before
    for retired in ["chunk_512B", "3KiB", "8MiB"]:
        case.command("snapshot", "capture", "--repo", repo, "--source", source,
                     "--chunk-preset", retired, "--yes", success=False)
    assert repository_inventory(repo) == before
    case.command("repo", "rename", "--repo", repo, "--name", "invalid/name", "--dry-run", success=False)
    case.stats("repo", "rename", "--repo", repo, "--name", "Preview name", "--dry-run")
    case.stats("repo", "constraints", "set", "--repo", repo, "--filesystem", "windows-ntfs", "--dry-run")
    assert repository_inventory(repo) == before
    snapshot = case.capture(repo, source, "--chunk-preset", "512B")["snapshot_id"]
    captured = repository_inventory(repo)
    case.stats("snapshot", "delete", "--repo", repo, "--snapshot", snapshot, "--dry-run", "--yes")
    case.stats("snapshot", "retag", "--repo", repo, "--snapshot", snapshot, "--tag", "version=2.0.0", "--dry-run", "--yes")
    case.stats("repo", "rebuild-index", "--repo", repo, "--publish", "--dry-run")
    assert repository_inventory(repo) == captured
    destination = case.payload / "installed"
    preview = case.stats("snapshot", "reconstruct", "--repo", repo,
                         "--snapshot", snapshot, "--destination", destination, "--dry-run")
    assert preview["summary"]["entryCount"] == 2
    assert not destination.exists()
    case.command("snapshot", "reconstruct", "--repo", repo, "--snapshot", snapshot,
                 "--destination", destination, "--staging-dir", destination, "--dry-run", success=False)
    assert not destination.exists()
    case.reconstruct(repo, snapshot, destination)
    (destination / "extra").write_bytes(b"keep")
    case.stats("installation", "repair", "--repo", repo, "--snapshot", snapshot,
               "--destination", destination, "--remove-extra-files", "--dry-run")
    assert (destination / "extra").read_bytes() == b"keep"
    events = case.payload / "preview.jsonl"
    case.command("installation", "plan", "--repo", repo, "--snapshot", snapshot,
                 "--destination", destination, "--json-lines", "--output", events)
    records = [json.loads(line) for line in events.read_text(encoding="utf-8").splitlines()]
    assert records[-1]["kind"] == "summary"
    assert any(record.get("path") == "extra" for record in records)
    for arguments in [("repo", "name", "set"), ("repo", "bind"),
                      ("repo", "sources", "capture"), ("installation", "corrupt")]:
        case.command(*arguments, success=False)
    case.checks.append("relative-paths-json-lines-dry-run-and-preset-validation")
