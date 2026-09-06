"""Operations: Hold a Windows file-sharing lock, attempt reconstruction, release the lock and
retry.

Expected: The failed promotion preserves old content and the retry installs exact output; other
platforms skip.
"""
import os
import unittest
from ..support.datasets import basic


def run(case):
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
