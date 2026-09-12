"""Operations: Replace blueprint directory paths with traversal, absolute and invalid-separator
paths.

Expected: Every reconstruction fails without modifying the run-owned filesystem.
"""
import struct
import os
from tests.datasets.generate import basic
from tests.helpers.filesystem import inventory


def run(case):
    repo = case.init()
    data = basic(case.payload / "source")
    snapshot = case.capture(repo, data.root)["snapshot_id"]
    blueprint = repo / "snapshots" / snapshot / "snapshot_blueprint.gtcbin"
    original = blueprint.read_bytes()
    paths = ["../outside", "/outside", "C:/outside", "nested\\outside", "a/./b",
             "a" * (128 * 1024 + 1), "/".join(["a"] * 257), "a" * 256]
    if os.name == "nt":
        paths += ["CON", "nul.txt", "name.", "name ", "stream:ads", "wild?card"]
    for path in paths:
        encoded = path.encode()
        # A single directory entry in the documented GUTCH_BLUEPRINT v1 framing.
        blueprint.write_bytes(b"GUTCH_BLUEPRINT\0\x01\0\0\0" + struct.pack("<IQBI", 1, 0, 1, len(encoded)) + encoded + b"\0")
        destination = case.payload / "installed"
        before = inventory(case.payload)
        case.command("snapshot", "reconstruct", "--repo", repo, "--snapshot-id", snapshot,
                     "--destination", destination, success=False)
        assert inventory(case.payload) == before
    blueprint.write_bytes(original)

    if os.name != "nt":
        from tests.helpers.filesystem import repository_inventory
        for name in (os.fsdecode(b"invalid-\xff"), "literal\\backslash"):
            bad = data.root / name
            bad.write_bytes(b"must not be silently renamed")
            before = repository_inventory(repo)
            case.command("snapshot", "capture", "--repo", repo, "--source-dir", data.root, success=False)
            assert repository_inventory(repo) == before
            bad.unlink()
