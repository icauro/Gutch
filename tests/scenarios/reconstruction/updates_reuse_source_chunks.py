"""Operations: Capture and install three versions using the previous installation as a source.

Expected: Each tree matches its target version and updates reuse source chunks.
"""

from tests.datasets.generate import basic


def run(case):
    repo = case.init()
    destination = case.payload / "installed"
    previous = None
    for version in (1, 2, 3):
        data = basic(case.payload / f"v{version}", version)
        captured = case.capture(repo, data.root)
        options = (
            []
            if previous is None
            else ["--reuse-snapshot", previous, "--reuse-from", destination]
        )
        result = case.reconstruct(repo, captured["snapshot_id"], destination, *options)
        case.compare(data, destination, f"version-{version}")
        if previous:
            assert result["reused_chunk_count"] > 0
        previous = captured["snapshot_id"]
