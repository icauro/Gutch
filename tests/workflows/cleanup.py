"""Destructive operations must preserve unrelated and still-referenced data."""
from ..support.datasets import basic
from ..support.filesystem import inventory


def repository_initialization(case):
    repo = case.payload / "nonempty"
    repo.mkdir()
    (repo / "keep.txt").write_bytes(b"unrelated")
    case.command("repo", "init", "--path", repo, success=False)
    assert (repo / "keep.txt").read_bytes() == b"unrelated"
    case.command("repo", "init", "--path", repo, "--force")
    assert (repo / "keep.txt").read_bytes() == b"unrelated"
    before = inventory(repo)
    case.command("repo", "init", "--path", repo, success=False)
    assert inventory(repo) == before


def snapshot_cleanup(case):
    repo = case.init()
    first_data = basic(case.payload / "v1")
    second_data = basic(case.payload / "v2", 2)
    first = case.capture(repo, first_data.root)["snapshot_id"]
    second = case.capture(repo, second_data.root)["snapshot_id"]
    index = repo / "chunk_map.gtcbin"
    original = index.read_bytes()
    for invalid in (None, b"GTCI\x01\0\0\0" + bytes(16)):
        index.unlink(missing_ok=True)
        if invalid is not None:
            index.write_bytes(invalid)
        before = inventory(repo)
        case.command("snapshot", "delete", "--repo", repo, "--snapshot-id", first, "--yes", success=False)
        assert inventory(repo) == before
    index.write_bytes(original)
    case.command("snapshot", "delete", "--repo", repo, "--snapshot-id", first, "--yes")
    destination = case.payload / "survivor"
    case.reconstruct(repo, second, destination)
    case.compare(second_data, destination)
    case.command("snapshot", "delete", "--repo", repo, "--snapshot-id", second, "--yes")
    assert not any(p.is_file() for p in (repo / "bundles").rglob("*"))


def recapture_missing_artifacts(case):
    repo = case.init()
    data = basic(case.payload / "source")
    first = case.capture(repo, data.root)["snapshot_id"]
    for path in (repo / "bundles").rglob("*"):
        if path.is_file():
            path.unlink()
    same = case.capture(repo, data.root)["snapshot_id"]
    assert first == same
    destination = case.payload / "identical"
    case.reconstruct(repo, same, destination)
    case.compare(data, destination, "identical-recapture")
    for path in (repo / "bundles").rglob("*"):
        if path.is_file():
            path.unlink()
    data.file("added.txt", b"new")
    changed = case.capture(repo, data.root)["snapshot_id"]
    assert changed != same
    case.reconstruct(repo, changed, destination)
    case.compare(data, destination, "changed-recapture")
