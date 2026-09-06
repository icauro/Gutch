"""Operations: Capture boundary-sized files with bundled and loose storage and three compression
modes.

Expected: Every reconstructed tree matches the independently specified input.
"""
from ..support.datasets import boundaries


def run(case):
    data = boundaries(case.payload / "source")
    for compression, storage in [("none", "bundled"), ("lz4", "bundled"), ("zstd", "loose_chunks")]:
        label = f"{compression}-{storage}"
        repo = case.init(label)
        captured = case.capture(repo, data.root, "--compression-algorithm", compression, "--storage-mode", storage)
        destination = case.payload / f"out-{label}"
        case.reconstruct(repo, captured["snapshot_id"], destination)
        case.compare(data, destination, label)
