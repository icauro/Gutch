"""Embedded notice contents, dependency selection, release staging and rejections."""

import hashlib
import json
import shutil

import pytest

from _tools.package_cli import ROOT, SOURCE_NOTICES, TARGETS, cli_dependencies, notices_text, stage_package


def metadata_for(root):
    upstream = root / "upstream"
    upstream.mkdir()
    (upstream / "LICENSE-MIT").write_text("shared MIT text", encoding="utf-8")
    (upstream / "vendor").mkdir()
    (upstream / "vendor/COPYING").write_text("native library notice", encoding="utf-8")
    sibling = root / "sibling"
    sibling.mkdir()
    (sibling / "LICENSE").write_text("shared MIT text\n", encoding="utf-8")
    return {
        "workspace_members": ["cli", "engine"],
        "packages": [
            {"id": "cli", "name": "gutch-cli"}, {"id": "engine", "name": "gutch"},
            {"id": "ffi", "name": "ffi", "version": "1.0", "license": "MIT",
             "manifest_path": str(upstream / "Cargo.toml")},
            {"id": "sibling", "name": "sibling", "version": "2.0", "license": "MIT",
             "manifest_path": str(sibling / "Cargo.toml")},
            {"id": "dev", "name": "dev-only", "version": "1.0"},
            {"id": "other-platform", "name": "other-platform", "version": "1.0"},
        ],
        "resolve": {"nodes": [
            {"id": "cli", "deps": [{"pkg": "engine", "dep_kinds": [{"kind": None}]}]},
            {"id": "engine", "deps": [
                {"pkg": "ffi", "dep_kinds": [{"kind": "build"}]},
                {"pkg": "sibling", "dep_kinds": [{"kind": None}]},
                {"pkg": "dev", "dep_kinds": [{"kind": "dev"}]},
            ]},
            {"id": "ffi", "deps": []}, {"id": "sibling", "deps": []}, {"id": "dev", "deps": []},
        ]},
    }


def prepare_root(root):
    for source in SOURCE_NOTICES:
        path = root / source
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / source, path)
    (root / ".secrets").mkdir()
    (root / ".secrets/private.env").write_text("must not be distributed")
    rust = root / "rust-notices"
    (rust / "licenses").mkdir(parents=True)
    (rust / "licenses/MIT.txt").write_text("Rust library license")
    (rust / "COPYRIGHT-library.html").write_text("<h1>Rust library</h1>\n<p>attribution &amp; terms</p>")
    return rust


def test_notices_cover_gutch_rust_and_each_distinct_dependency_text_once(tmp_path):
    rust = prepare_root(tmp_path)
    metadata = metadata_for(tmp_path)
    assert [package["id"] for package in cli_dependencies(metadata)] == ["ffi", "sibling"]
    text = notices_text(tmp_path, TARGETS[0], metadata, rust)
    assert (ROOT / "LICENSE").read_text(encoding="utf-8").strip() in text
    assert "Rust library license" in text
    assert "attribution & terms" in text and "<p>" not in text
    assert "native library notice" in text
    assert text.count("shared MIT text") == 1
    assert "ffi 1.0 (MIT): LICENSE-MIT\n  sibling 2.0 (MIT): LICENSE" in text
    assert "dev-only" not in text and "must not be distributed" not in text


def test_notices_reject_a_dependency_without_notices(tmp_path):
    rust = prepare_root(tmp_path)
    metadata = metadata_for(tmp_path)
    shutil.rmtree(tmp_path / "upstream")
    with pytest.raises(ValueError, match="No license/notice files found"):
        notices_text(tmp_path, TARGETS[0], metadata, rust)


@pytest.mark.parametrize("target", TARGETS)
def test_package_holds_only_the_executable_its_provenance_and_checksum(tmp_path, target):
    name = "gutch-cli.exe" if "windows" in target else "gutch-cli"
    binary = tmp_path / ".builds/compiler-cache/cargo" / target / "release" / name
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b"mock CLI")
    metadata = metadata_for(tmp_path)
    output = stage_package(tmp_path, target, metadata, {"commit": "abc", "compiler": "rustc test"})
    assert sorted(path.name for path in output.iterdir()) == sorted([name, "SHA256SUMS", "build.json"])
    assert (output / name).read_bytes() == b"mock CLI"
    record = json.loads((output / "build.json").read_text(encoding="utf-8"))
    assert record["commit"] == "abc" and [d["name"] for d in record["dependencies"]] == ["ffi", "sibling"]
    assert (output / "SHA256SUMS").read_text(encoding="utf-8") == f"{hashlib.sha256(b'mock CLI').hexdigest()}  {name}\n"
    (output / "stale.txt").touch()
    stage_package(tmp_path, target, metadata, {"commit": "abc"})
    assert not (output / "stale.txt").exists()


def test_unsupported_targets_are_rejected(tmp_path):
    with pytest.raises(ValueError, match="Unsupported distribution target"):
        stage_package(tmp_path, "../../../other", {}, {})
    with pytest.raises(ValueError, match="Unsupported distribution target"):
        notices_text(tmp_path, "../../../other", {}, None)
