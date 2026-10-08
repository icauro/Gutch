"""Operations: Hold a Windows file-sharing lock, attempt reconstruction, release the lock and
retry.

Expected: The failed promotion preserves old content and the retry installs exact output; other
platforms skip.
"""
import os
import json
import unittest
from tests.datasets.generate import basic
from tests.helpers.filesystem import inventory


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
    (destination / "other").mkdir()
    (destination / "other" / "saved.txt").write_bytes(b"also preserve")
    before = inventory(destination)
    create_file = ctypes.windll.kernel32.CreateFileW
    create_file.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    create_file.restype = wintypes.HANDLE
    handle = create_file(str(target), 0x80000000, 1, None, 3, 0, None)
    assert handle != wintypes.HANDLE(-1).value
    try:
        error_path = case.root / "promotion-error.json"
        output_path = case.root / "promotion-output.json"
        case.command("installation", "reconstruct", "--repo", repo, "--snapshot", snapshot,
                     "--destination", destination, "--json", "--output", output_path,
                     "--error-json-out", error_path, success=False)
        failure = json.loads(error_path.read_text(encoding="utf-8"))
        summary = json.loads(output_path.read_text(encoding="utf-8"))
        assert summary["error_details"] == failure
        assert failure["code"] == "directory_promotion_failed"
        details = failure["details"]
        assert details["phase"] == "move_destination_to_backup"
        assert details["committed"] is False
        assert details["destination"]["exists"] is True
        assert details["staging"]["exists"] is True
        assert details["backup"]["exists"] is False
        assert isinstance(details["primary_error"]["raw_os_error"], int)
        assert details["observed_at"].endswith("Z")
        assert details["recovery"] == []
        assert inventory(destination) == before
        case.command("installation", "reconstruct", "--repo", repo, "--snapshot", snapshot,
                     "--destination", destination, "--json-lines", "--output", output_path,
                     success=False)
        lines = [json.loads(line) for line in output_path.read_text(encoding="utf-8").splitlines()]
        assert lines[-1]["kind"] == "summary"
        assert lines[-1]["summary"]["error_details"]["code"] == "directory_promotion_failed"
        # A diagnostic export error must not replace the promotion failure.
        unavailable = case.payload / "not-a-directory"
        unavailable.write_text("block output parent", encoding="utf-8")
        case.command("installation", "reconstruct", "--repo", repo, "--snapshot", snapshot,
                     "--destination", destination, "--json", "--output", output_path,
                     "--error-json-out", unavailable / "error.json", success=False)
        summary = json.loads(output_path.read_text(encoding="utf-8"))
        assert summary["error_details"]["code"] == "directory_promotion_failed"
        assert "Error details could not be saved" in (case.root / "commands" / f"{case.sequence:03d}-installation.log").read_text(encoding="utf-8")
        assert inventory(destination) == before
    finally:
        ctypes.windll.kernel32.CloseHandle(wintypes.HANDLE(handle))
    case.reconstruct(repo, snapshot, destination)
    case.compare(data, destination, "retry-after-promotion-failure")
