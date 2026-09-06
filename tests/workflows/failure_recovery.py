"""Invalid paths and occupied destinations fail without destroying committed content."""
import os
import struct
import unittest

from ..support.datasets import basic
from ..support.filesystem import inventory


def staging_safety(case):
    repo = case.init()
    data = basic(case.payload / "source")
    snapshot = case.capture(repo, data.root)["snapshot_id"]
    destination = case.payload / "installation" / "content"
    destination.mkdir(parents=True)
    (destination / "keep.txt").write_bytes(b"installed")
    for staging in (destination, destination.parent, destination / "child", destination / ".." / "content"):
        before = inventory(destination.parent)
        for command in [("snapshot", "reconstruct"), ("reconstructed", "repair")]:
            case.command(*command, "--repo", repo, "--snapshot-id", snapshot,
                "--destination", destination, "--staging-dir", staging,
                *(["--yes"] if command[1] == "repair" else []), success=False)
            assert inventory(destination.parent) == before
    staging = case.payload / "work"
    staging.mkdir()
    (staging / "keep.txt").write_bytes(b"unrelated")
    case.reconstruct(repo, snapshot, destination, "--staging-dir", staging)
    case.compare(data, destination)
    assert list(staging.iterdir()) == [staging / "keep.txt"]


def staging_link_alias(case):
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


def unsafe_blueprint(case):
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


def failed_promotion(case):
    if os.name != "nt":
        raise unittest.SkipTest("Windows sharing-denial promotion test; Unix rename does not use sharing locks")
    import ctypes
    from ctypes import wintypes
    repo = case.init()
    data = basic(case.payload / "source")
    snapshot = case.capture(repo, data.root)["snapshot_id"]
    destination = case.payload / "installed"
    destination.mkdir()
    target = destination / "keep.txt"
    target.write_bytes(b"old installation")
    create_file = ctypes.windll.kernel32.CreateFileW
    create_file.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    create_file.restype = wintypes.HANDLE
    handle = create_file(str(target), 0x80000000, 1, None, 3, 0, None)
    assert handle != wintypes.HANDLE(-1).value
    try:
        case.command("snapshot", "reconstruct", "--repo", repo, "--snapshot-id", snapshot,
                     "--destination", destination, success=False)
        assert target.read_bytes() == b"old installation"
    finally:
        ctypes.windll.kernel32.CloseHandle(wintypes.HANDLE(handle))
    case.reconstruct(repo, snapshot, destination)
    case.compare(data, destination, "retry-after-promotion-failure")
