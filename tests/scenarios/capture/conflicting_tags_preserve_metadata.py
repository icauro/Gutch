"""Operations: Recapture identical content with a conflicting version tag.

Expected: Capture fails and the existing snapshot metadata stays byte-for-byte unchanged and its content still reconstructs.
"""
from tests.datasets.trees import basic


def run(case):
    data = basic(case.payload / "source")
    repo = case.init()
    captured = case.capture(repo, data.root, "--tag", "version::1.0.0")
    path = repo / "snapshots" / captured["snapshot_id"] / "snapshot_metadata.json"
    before = path.read_bytes()
    case.command("snapshot", "capture", "--repo", repo, "--source-dir", data.root,
                 "--tag", "version::2.0.0", "--yes", success=False)
    assert path.read_bytes() == before
    destination = case.payload / "preserved"
    case.reconstruct(repo, captured["snapshot_id"], destination)
    case.compare(data, destination)
