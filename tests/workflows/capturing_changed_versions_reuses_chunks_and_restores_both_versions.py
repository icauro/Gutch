"""Operations: Capture two versions with shared content and reconstruct each.

Expected: Snapshot IDs differ, chunks are reused, and both versions match their inputs.
"""
from ..support.datasets import basic


def run(case):
    repo = case.init()
    first = case.capture(repo, basic(case.payload / "v1").root)
    data = basic(case.payload / "v2", 2)
    second = case.capture(repo, data.root)
    assert first["snapshot_id"] != second["snapshot_id"]
    assert second["reused_chunk_count"] > 0
    for label, stats, version in [("v1", first, 1), ("v2", second, 2)]:
        destination = case.payload / f"install-{label}"
        case.reconstruct(repo, stats["snapshot_id"], destination)
        case.compare(basic(case.payload / f"expected-{label}", version), destination, label)
