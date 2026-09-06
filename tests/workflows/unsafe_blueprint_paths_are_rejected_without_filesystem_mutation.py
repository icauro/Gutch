"""Operations: Replace blueprint directory paths with traversal, absolute and invalid-separator
paths.

Expected: Every reconstruction fails without modifying the run-owned filesystem.
"""
import struct
from ..support.datasets import basic
from ..support.filesystem import inventory


def run(case):
    repo = case.init()
    data = basic(case.payload / "source")
    snapshot = case.capture(repo, data.root)["snapshot_id"]
    blueprint = repo / "snapshots" / snapshot / "snapshot_blueprint.gtcbin"
    original = blueprint.read_bytes()
    for path in ("../outside", "/outside", "C:/outside", "nested\\outside"):
        encoded = path.encode()
        # A single directory entry in the documented GTCT v1 framing.
        blueprint.write_bytes(b"GTCT\x01\0\0\0" + struct.pack("<IQBI", 1, 0, 1, len(encoded)) + encoded + b"\0")
        destination = case.payload / "installed"
        before = inventory(case.payload)
        case.command("snapshot", "reconstruct", "--repo", repo, "--snapshot-id", snapshot,
                     "--destination", destination, success=False)
        assert inventory(case.payload) == before
    blueprint.write_bytes(original)
