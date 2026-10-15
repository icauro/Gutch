"""Operations: Try overlapping staging paths for reconstruction and repair, then a disjoint staging
parent.

Expected: Overlap fails without mutation; disjoint staging works and preserves preexisting
files.
"""

from tests.datasets.generate import basic
from tests.helpers.filesystem import inventory


def run(case):
    repo = case.init()
    data = basic(case.payload / "source")
    snapshot = case.capture(repo, data.root)["snapshot_id"]
    destination = case.payload / "installation" / "content"
    destination.mkdir(parents=True)
    (destination / "keep.txt").write_bytes(b"installed")
    for staging in (
        destination,
        destination.parent,
        destination / "child",
        destination / ".." / "content",
    ):
        before = inventory(destination.parent)
        for command in [("installation", "reconstruct"), ("installation", "repair")]:
            case.command(
                *command,
                "--repo",
                repo,
                "--snapshot",
                snapshot,
                "--destination",
                destination,
                "--staging-dir",
                staging,
                *(["--yes"] if command[1] == "repair" else []),
                success=False,
            )
            assert inventory(destination.parent) == before
    staging = case.payload / "work"
    staging.mkdir()
    (staging / "keep.txt").write_bytes(b"unrelated")
    case.reconstruct(repo, snapshot, destination, "--staging-dir", staging)
    case.compare(data, destination)
    assert list(staging.iterdir()) == [staging / "keep.txt"]
