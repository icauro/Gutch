"""Verification classifications and repair, after independently validating installation."""
from ..support.datasets import metadata


def verify_and_repair(case):
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
    case.repair(repo, snapshot, destination)
    assert (destination / "extra.txt").read_bytes() == b"user content"
    (destination / "extra.txt").unlink()
    case.compare(data, destination, "repaired-preserving-extra")
    (destination / "extra.txt").write_bytes(b"user content")
    case.repair(repo, snapshot, destination, "--remove-added-files")
    case.compare(data, destination, "repaired-removing-extra")
