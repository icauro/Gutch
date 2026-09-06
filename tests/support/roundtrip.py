"""Capture and independently compare filesystem metadata."""
from .datasets import basic, metadata


def metadata_roundtrip(case, *, links):
    data = metadata(case.payload / "source", links=links)
    repo = case.init()
    captured = case.capture(repo, data.root)
    destination = case.payload / "installed"
    case.reconstruct(repo, captured["snapshot_id"], destination)
    case.compare(data, destination)


def capture_roundtrip(case):
    data = basic(case.payload / "source")
    repo = case.init()
    first = case.capture(repo, data.root)
    again = case.capture(repo, data.root)
    assert first["snapshot_id"] == again["snapshot_id"]
    assert again["generated_bundle_count"] == 0
    destination = case.payload / "installed"
    case.reconstruct(repo, first["snapshot_id"], destination)
    case.compare(data, destination)
    assert case.verify(repo, first["snapshot_id"], destination)["verification_passed"]
