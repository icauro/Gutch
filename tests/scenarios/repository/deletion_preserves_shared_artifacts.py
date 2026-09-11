"""Operations: Capture two versions, attempt deletion with broken indexes, then delete each
normally.

Expected: Broken indexes prevent mutation; the surviving snapshot remains usable and final
deletion removes bundles.
"""
from tests.support.datasets import basic
from tests.support.filesystem import repository_inventory as inventory


def run(case):
    repo = case.init()
    first_data = basic(case.payload / "v1")
    second_data = basic(case.payload / "v2", 2)
    first = case.capture(repo, first_data.root)["snapshot_id"]
    second_capture = case.capture(repo, second_data.root)
    assert second_capture["reused_chunk_count"] > 0
    second = second_capture["snapshot_id"]
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
