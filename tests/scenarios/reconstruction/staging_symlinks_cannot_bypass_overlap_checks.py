"""Operations: Point a staging-directory symlink at the installation and attempt reconstruction.

Expected: The operation fails and installed content remains unchanged; unsupported hosts
explicitly skip.
"""
import unittest
from tests.datasets.trees import basic
from tests.helpers.filesystem import inventory


def run(case):
    repo = case.init()
    data = basic(case.payload / "source")
    snapshot = case.capture(repo, data.root)["snapshot_id"]
    destination = case.payload / "installed"
    destination.mkdir()
    (destination / "keep").write_bytes(b"keep")
    alias = case.payload / "alias"
    try:
        alias.symlink_to(destination, target_is_directory=True)
    except OSError as error:
        raise unittest.SkipTest(f"directory links unavailable: {error}") from error
    before = inventory(destination)
    case.command("snapshot", "reconstruct", "--repo", repo, "--snapshot-id", snapshot,
                 "--destination", destination, "--staging-dir", alias, success=False)
    assert inventory(destination) == before
