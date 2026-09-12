"""Frozen input coverage, bounded fuzz generation and independent provenance."""
import hashlib
import json
from pathlib import Path

import pytest

from tests.datasets.generate import CORPUS_ROOT, FUZZ_PROFILES, case_names, fuzzed, load_case
from tests.helpers.corpus import freeze, repository_input


@pytest.mark.parametrize("profile", FUZZ_PROFILES)
def test_frozen_generated_sources_reproduce_from_saved_fuzzer_input(tmp_path, profile):
    name = next(name for name in case_names() if load_case(name)["source"].get("profile") == profile)
    definition = load_case(name)
    settings = definition["source"]
    expected = json.loads((CORPUS_ROOT / name / "expected.json").read_text(encoding="utf-8"))
    for version in range(1, settings["versions"] + 1):
        generated = fuzzed(tmp_path / f"v{version}", bytes.fromhex(settings["input_hex"]),
                          profile=profile, version=version, max_bytes=settings["max_bytes_per_version"])
        entries = expected[f"v{version}"]["entries"]
        assert generated.expected.keys() == entries.keys()
        for path, entry in entries.items():
            if entry["kind"] == "file":
                assert generated.expected[path]["sha256"] == entry["sha256"]
                committed = CORPUS_ROOT / name / "source" / f"v{version}" / path
                assert hashlib.sha256(committed.read_bytes()).hexdigest() == entry["sha256"]
        assert sum(entry["size"] for entry in entries.values() if entry["kind"] == "file") <= settings["max_bytes_per_version"]
    assert sum(path.stat().st_size for path in (CORPUS_ROOT / name).rglob("*") if path.is_file()) <= 16 * 1024 * 1024


def test_every_case_has_a_frozen_major_version_and_complete_inventory():
    generated = [name for name in case_names() if load_case(name)["source"]["kind"] == "generated"]
    assert len(generated) == 10
    for name in case_names():
        repo, expected = repository_input(name, "v2")
        assert (repo / "repo_metadata.json").is_file()
        assert {path.name for path in (repo / "snapshots").iterdir() if path.is_dir()} == expected.keys()
    assert not (CORPUS_ROOT / "blender/source").exists()


def test_freezing_an_existing_major_version_is_rejected_before_execution():
    with pytest.raises(ValueError, match="already exists"):
        freeze("blender", "v2", Path("missing-executable"))


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
    assert any(any(ord(c) > 0xffff for c in name) for name in first.expected)
    assert sum(e.get("size", 0) for e in first.expected.values()) <= 65536


def test_versions_include_metadata_only_and_type_changes(tmp_path):
    first = fuzzed(tmp_path / "first", b"changes", version=1)
    second = fuzzed(tmp_path / "second", b"changes", version=2)
    assert first.expected["permission-change.txt"]["sha256"] == second.expected["permission-change.txt"]["sha256"]
    assert first.expected["permission-change.txt"]["read_only"] != second.expected["permission-change.txt"]["read_only"]
    assert first.expected["type-change"]["kind"] == "file"
    assert second.expected["type-change"]["kind"] == "directory"


def test_operation_reducer_preserves_failure_and_removes_irrelevant_actions():
    from tests.helpers.minimize import reduce_sequence
    original = ["capture", "repair", "delete", "rebuild", "repair"]
    def fails(actions):
        return "delete" in actions and "rebuild" in actions and actions.index("delete") < actions.index("rebuild")
    assert reduce_sequence(original, fails) == ["delete", "rebuild"]
    assert original == ["capture", "repair", "delete", "rebuild", "repair"]
