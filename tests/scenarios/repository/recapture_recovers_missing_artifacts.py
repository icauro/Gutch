"""Operations: Delete captured bundles, recapture identical content, then repeat with changed
content, using different chunking settings for repair.

Expected: Missing artifacts are rebuilt; identical content keeps its ID and both outputs
reconstruct correctly.
"""
from tests.support.datasets import basic


def run(case):
    repo = case.init()
    data = basic(case.payload / "source")
    first = case.capture(repo, data.root)["snapshot_id"]
    for path in (repo / "bundles").rglob("*"):
        if path.is_file():
            path.unlink()
    same = case.capture(repo, data.root, "--chunk-profile", "chunk_16KiB")["snapshot_id"]
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
