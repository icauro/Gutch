"""Frozen input coverage, bounded fuzz generation and independent provenance."""

import hashlib
import json
from pathlib import Path

import pytest

from tests.datasets.generate import (
    CORPUS_ROOT,
    FUZZ_PROFILES,
    case_names,
    fuzzed,
    load_case,
)
from tests.helpers.corpus import freeze, repository_input


@pytest.mark.parametrize("profile", FUZZ_PROFILES)
def test_frozen_generated_sources_reproduce_from_saved_fuzzer_input(tmp_path, profile):
    name = next(
        name
        for name in case_names()
        if load_case(name)["source"].get("profile") == profile
    )
    definition = load_case(name)
    settings = definition["source"]
    expected = json.loads(
        (CORPUS_ROOT / name / "expected.json").read_text(encoding="utf-8")
    )
    for version in range(1, settings["versions"] + 1):
        generated = fuzzed(
            tmp_path / f"v{version}",
            bytes.fromhex(settings["input_hex"]),
            profile=profile,
            version=version,
            max_bytes=settings["max_bytes_per_version"],
        )
        entries = expected[f"v{version}"]["entries"]
        assert generated.expected.keys() == entries.keys()
        for path, entry in entries.items():
            if entry["kind"] == "file":
                assert generated.expected[path]["sha256"] == entry["sha256"]
                committed = CORPUS_ROOT / name / "source" / f"v{version}" / path
                assert (
                    hashlib.sha256(committed.read_bytes()).hexdigest()
                    == entry["sha256"]
                )
        assert (
            sum(entry["size"] for entry in entries.values() if entry["kind"] == "file")
            <= settings["max_bytes_per_version"]
        )
    assert (
        sum(
            path.stat().st_size
            for path in (CORPUS_ROOT / name).rglob("*")
            if path.is_file()
        )
        <= 16 * 1024 * 1024
    )


def test_every_case_has_a_frozen_major_version_and_complete_inventory():
    generated = [
        name
        for name in case_names()
        if load_case(name)["source"]["kind"] == "generated"
    ]
    assert len(generated) == 10
    for name in generated:
        repo, expected = repository_input(name, "v1")
        assert (repo / "repo_metadata.json").is_file()
        assert {
            path.name for path in (repo / "snapshots").iterdir() if path.is_dir()
        } == expected.keys()
    assert not (CORPUS_ROOT / "blender/source").exists()
    assert not (CORPUS_ROOT / "blender/repository").exists()
    definition = load_case("blender")
    assert definition["repositories"]["v1"]["preparation"] == "capture"
    expected = json.loads(
        (CORPUS_ROOT / "blender/expected.json").read_text(encoding="utf-8")
    )
    assert set(definition["repositories"]["v1"]["snapshots"].values()) == expected.keys()


def test_freezing_an_existing_major_version_is_rejected_before_execution():
    with pytest.raises(ValueError, match="already exists"):
        freeze("blender", "v1", Path("missing-executable"))
    with pytest.raises(ValueError, match="disposable capture"):
        freeze("blender", "v2", Path("missing-executable"))


def test_blender_preparation_keeps_pinned_expectations_and_rejects_changed_cache(
    tmp_path, monkeypatch
):
    from types import SimpleNamespace
    import shutil
    import zipfile
    from tests.datasets import generate, prepare_blender
    from tests.helpers import corpus
    from tests.helpers.filesystem import inventory

    root = tmp_path / "corpus"
    case_root = root / "blender"
    case_root.mkdir(parents=True)
    folder = "blender-5.2.0-windows-x64"
    original = tmp_path / "original"
    original.mkdir()
    (original / "blender.exe").write_bytes(b"independent source bytes")
    archive = tmp_path / f"{folder}.zip"
    with zipfile.ZipFile(archive, "w") as output:
        output.write(original / "blender.exe", f"{folder}/blender.exe")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    expected = {folder: {"entries": inventory(original)}}
    definition = load_case("blender")
    definition["source"].update(
        selected_versions=["5.2.0"],
        versions=[{
            "version": "5.2.0",
            "assets": [{
                "archive": archive.name,
                "platform": "windows-x64",
                "sha256": digest,
            }],
        }],
    )
    snapshot = "1" * 64
    definition["repositories"]["v1"]["snapshots"] = {snapshot: folder}
    (case_root / "case.json").write_text(json.dumps(definition))
    expected_path = case_root / "expected.json"
    expected_path.write_text(json.dumps(expected))
    before = expected_path.read_bytes()
    monkeypatch.setattr(generate, "CORPUS_ROOT", root)
    monkeypatch.setattr(generate, "TESTS_ROOT", tmp_path / "tests")
    monkeypatch.setattr(corpus, "CORPUS_ROOT", root)
    downloads = []

    def download(asset, destination, base_url):
        downloads.append(asset.archive_name)
        shutil.copyfile(archive, destination)

    monkeypatch.setattr(prepare_blender, "_download_blender_asset", download)
    captures = []

    class CaptureCase:
        def __init__(self, root, executable):
            self.root = root

        def init(self):
            return self.root / "repo"

        def capture(self, repo, source, *options):
            captures.append(source)
            return {"snapshot_id": snapshot}

    monkeypatch.setattr(corpus, "Case", CaptureCase)
    case = SimpleNamespace(payload=tmp_path / "run/payload", executable=Path("cli"))
    repo, observed = repository_input("blender", case=case)
    assert repo.is_relative_to(case.payload)
    assert observed == {snapshot: expected[folder]}
    assert downloads == [archive.name]
    assert len(captures) == 1
    repository_input("blender", case=case)
    assert downloads == [archive.name]  # Verified archive is reused offline.
    assert expected_path.read_bytes() == before

    definition["repositories"]["v1"]["snapshots"] = {"2" * 64: folder}
    (case_root / "case.json").write_text(json.dumps(definition))
    with pytest.raises(AssertionError, match="identity differs"):
        repository_input("blender", case=case)
    captures.clear()
    cached_file = (
        tmp_path / "tests/.artifacts/datasets/blender/source" / folder / "blender.exe"
    )
    cached_file.write_bytes(b"tampered cache")
    with pytest.raises(ValueError, match="source differs"):
        repository_input("blender", case=case)
    assert not captures
    assert expected_path.read_bytes() == before


@pytest.mark.parametrize("budget", [0, 127, 4 * 1024 * 1024 + 1])
def test_fuzzer_cannot_exceed_resource_limits(tmp_path, budget):
    with pytest.raises(ValueError, match="budget"):
        fuzzed(tmp_path, b"arbitrary input", max_bytes=budget)


def test_random_names_and_attributes_change_with_seed_and_preserve_unicode(tmp_path):
    import unicodedata
    from tests.helpers.filesystem import inventory

    first = fuzzed(tmp_path / "first", b"one", max_bytes=65536)
    second = fuzzed(tmp_path / "second", b"two", max_bytes=65536)
    assert first.expected.keys() != second.expected.keys()
    assert first.expected == inventory(first.root)
    assert any(entry.get("size") == 0 for entry in first.expected.values())
    assert any(entry.get("read_only") for entry in first.expected.values())
    assert any(unicodedata.normalize("NFC", name) != name for name in first.expected)
    assert any(any(ord(c) > 0xFFFF for c in name) for name in first.expected)
    assert sum(e.get("size", 0) for e in first.expected.values()) <= 65536


def test_versions_include_metadata_only_and_type_changes(tmp_path):
    first = fuzzed(tmp_path / "first", b"changes", version=1)
    second = fuzzed(tmp_path / "second", b"changes", version=2)
    assert (
        first.expected["permission-change.txt"]["sha256"]
        == second.expected["permission-change.txt"]["sha256"]
    )
    assert (
        first.expected["permission-change.txt"]["read_only"]
        != second.expected["permission-change.txt"]["read_only"]
    )
    assert first.expected["type-change"]["kind"] == "file"
    assert second.expected["type-change"]["kind"] == "directory"


def test_operation_reducer_preserves_failure_and_removes_irrelevant_actions():
    from tests.helpers.minimize import reduce_sequence

    original = ["capture", "repair", "delete", "rebuild", "repair"]

    def fails(actions):
        return (
            "delete" in actions
            and "rebuild" in actions
            and actions.index("delete") < actions.index("rebuild")
        )

    assert reduce_sequence(original, fails) == ["delete", "rebuild"]
    assert original == ["capture", "repair", "delete", "rebuild", "repair"]
