"""Operations: Capture boundary-sized files with bundled and loose storage and three compression
modes.

Expected: Every profile gives the same installation ID and output despite different chunk layouts;
healthy recapture retains its existing blueprint.
"""
from ..support.datasets import boundaries


def run(case):
    data = boundaries(case.payload / "source")
    identities = set()
    chunk_counts = set()
    for compression, storage, chunking in [("none", "bundled", "chunk_16KiB"), ("lz4", "bundled", "chunk_1MiB"), ("zstd", "loose_chunks", "chunk_128KiB")]:
        label = f"{compression}-{storage}"
        repo = case.init(label)
        captured = case.capture(repo, data.root, "--compression-algorithm", compression, "--storage-mode", storage, "--chunk-profile", chunking)
        identities.add(captured["snapshot_id"])
        chunk_counts.add(captured["required_chunk_count"])
        blueprint = repo / "snapshots" / captured["snapshot_id"] / "snapshot_blueprint.gtcbin"
        before = blueprint.read_bytes()
        again = case.capture(repo, data.root, "--chunk-profile", "chunk_32KiB", "--compression-algorithm", compression, "--storage-mode", storage)
        assert again["snapshot_id"] == captured["snapshot_id"]
        assert again["generated_bundle_count"] == 0
        assert blueprint.read_bytes() == before
        destination = case.payload / f"out-{label}"
        case.reconstruct(repo, captured["snapshot_id"], destination)
        case.compare(data, destination, label)
    assert len(identities) == 1
    assert len(chunk_counts) > 1, "fixture must exercise different chunk layouts"
