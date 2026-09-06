"""Installation updates and filesystem metadata, checked independently."""
from ..support.datasets import basic, metadata


def update_and_source_reuse(case):
    repo = case.init()
    destination = case.payload / "installed"
    previous = None
    for version in (1, 2, 3):
        data = basic(case.payload / f"v{version}", version)
        captured = case.capture(repo, data.root)
        options = [] if previous is None else ["--source-snapshot-id", previous, "--source-dir", destination]
        result = case.reconstruct(repo, captured["snapshot_id"], destination, *options)
        case.compare(data, destination, f"version-{version}")
        if previous:
            assert result["reused_source_chunk_count"] > 0
        previous = captured["snapshot_id"]


def filesystem_metadata(case):
    _metadata_roundtrip(case, links=False)


def symbolic_links(case):
    _metadata_roundtrip(case, links=True)


def _metadata_roundtrip(case, *, links):
    data = metadata(case.payload / "source", links=links)
    repo = case.init()
    captured = case.capture(repo, data.root)
    destination = case.payload / "installed"
    case.reconstruct(repo, captured["snapshot_id"], destination)
    case.compare(data, destination)
