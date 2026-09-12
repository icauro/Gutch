"""Capture and independently compare filesystem metadata."""
from tests.datasets.generate import basic, metadata
from .filesystem import remove_owned


def metadata_roundtrip(case, *, links):
    data = metadata(case.payload / "source", links=links)
    data.file("a" * 255, b"maximum ASCII component")
    data.file("é" * 127 + "x", b"255 UTF-8 bytes")
    repo = case.init()
    captured = case.capture(repo, data.root)
    destination = case.payload / "installed"
    case.reconstruct(repo, captured["snapshot_id"], destination)
    case.compare(data, destination)
    if links:
        (data.root / "shared-link").unlink()
        data.link("shared-link", "data/duplicate.bin")
    else:
        (data.root / "readonly.txt").chmod(0o600)
        data.expected["readonly.txt"]["read_only"] = False
    changed = case.capture(repo, data.root)
    updated = case.payload / "updated"
    case.reconstruct(repo, changed["snapshot_id"], updated)
    case.compare(data, updated, "metadata-only-change")


def capture_roundtrip(case):
    data = basic(case.payload / "source")
    repo = case.init()
    first = case.capture(repo, data.root)
    again = case.capture(repo, data.root)
    assert first["snapshot_id"] == again["snapshot_id"]
    assert again["generated_bundle_count"] == 0
    remove_owned(data.root, case.payload)
    destination = case.payload / "installed"
    case.reconstruct(repo, first["snapshot_id"], destination)
    case.compare(data, destination)
    assert case.verify(repo, first["snapshot_id"], destination)["verification_passed"]
