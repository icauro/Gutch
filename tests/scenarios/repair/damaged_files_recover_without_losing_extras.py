"""Operations: Install, corrupt content and attributes, delete a file, add a user file, then repair
twice.

Expected: Verification classifies the damage; repair restores input and removes extras only when
requested.
"""
import json
from tests.datasets.trees import metadata


def run(case):
    data = metadata(case.payload / "source")
    repo = case.init()
    snapshot = case.capture(repo, data.root)["snapshot_id"]
    destination = case.payload / "installed"
    case.reconstruct(repo, snapshot, destination)
    case.compare(data, destination, "before-corruption")
    (destination / "data/shared.bin").write_bytes(b"truncated")
    (destination / "old.txt").unlink()
    (destination / "readonly.txt").chmod(0o600)
    (destination / "extra.txt").write_bytes(b"user content")
    result = case.verify(repo, snapshot, destination, success=False)
    assert not result["verification_passed"]
    assert result["missing_file_count"] >= 1
    assert result["corrupt_file_count"] >= 2
    assert result["extra_file_count"] == 1
    case.command("reconstructed", "plan", "--repo", repo, "--snapshot-id", snapshot,
                 "--destination", destination)
    preview = case.root / "commands" / f"{case.sequence:03d}-reconstructed.log"
    events = [json.loads(line) for line in preview.read_text().splitlines() if line.startswith("{")]
    actions = {event["path"]: event["action"] for event in events if event["kind"] == "entry"}
    assert actions["data/shared.bin"] == "replace"
    assert actions["old.txt"] == "add"
    assert actions["extra.txt"] == "preserveExtra"
    assert events[-1]["kind"] == "summary"
    assert (destination / "data/shared.bin").read_bytes() == b"truncated"
    case.repair(repo, snapshot, destination)
    assert (destination / "extra.txt").read_bytes() == b"user content"
    (destination / "extra.txt").unlink()
    case.compare(data, destination, "repaired-preserving-extra")
    (destination / "extra.txt").write_bytes(b"user content")
    case.repair(repo, snapshot, destination, "--remove-added-files")
    case.compare(data, destination, "repaired-removing-extra")
