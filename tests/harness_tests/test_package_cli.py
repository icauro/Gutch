"""Distribution contents, dependency selection and missing-notice rejection."""

import hashlib
import json
from pathlib import Path
import shutil

import pytest

from tools.package_cli import ROOT, SOURCE_NOTICES, TARGETS, cli_dependencies, stage_package


def metadata_for(root):
    upstream = root / "upstream"
    upstream.mkdir()
    (upstream / "LICENSE-MIT").write_text("upstream license", encoding="utf-8")
    (upstream / "vendor").mkdir()
    (upstream / "vendor/COPYING").write_text("native library notice", encoding="utf-8")
    return {
        "workspace_members": ["cli", "engine"],
        "packages": [
            {"id": "cli", "name": "gutch-cli"}, {"id": "engine", "name": "gutch"},
            {"id": "ffi", "name": "ffi", "version": "1.0", "license": "MIT",
             "manifest_path": str(upstream / "Cargo.toml")},
            {"id": "dev", "name": "dev-only", "version": "1.0"},
            {"id": "other-platform", "name": "other-platform", "version": "1.0"},
        ],
        "resolve": {"nodes": [
            {"id": "cli", "deps": [{"pkg": "engine", "dep_kinds": [{"kind": None}]}]},
            {"id": "engine", "deps": [
                {"pkg": "ffi", "dep_kinds": [{"kind": "build"}]},
                {"pkg": "dev", "dep_kinds": [{"kind": "dev"}]},
            ]},
            {"id": "ffi", "deps": []}, {"id": "dev", "deps": []},
        ]},
    }


def prepare_root(root, target):
    name = "gutch-cli.exe" if "windows" in target else "gutch-cli"
    binary = root / ".builds/compiler-cache/cargo" / target / "release" / name
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b"mock CLI")
    for source in SOURCE_NOTICES:
        path = root / source
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / source, path)
    (root / ".secrets").mkdir()
    (root / ".secrets/private.env").write_text("must not be distributed")
    notices = root / "rust-notices"
    (notices / "licenses").mkdir(parents=True)
    (notices / "licenses/MIT.txt").write_text("Rust library license")
    (notices / "COPYRIGHT-library.html").write_text("Rust library attribution")
    return binary


@pytest.mark.parametrize("target", TARGETS)
def test_package_contains_binary_checksums_and_native_notices_only(tmp_path, target):
    binary = prepare_root(tmp_path, target)
    metadata = metadata_for(tmp_path)
    assert [package["id"] for package in cli_dependencies(metadata)] == ["ffi"]
    output = stage_package(tmp_path, target, metadata, {"commit": "abc", "compiler": "rustc test"}, tmp_path / "rust-notices")
    assert (output / binary.name).read_bytes() == b"mock CLI"
    assert (output / "licenses/dependencies/ffi-1.0/vendor/COPYING").read_text() == "native library notice"
    assert (output / "licenses/rust/COPYRIGHT-library.html").read_text() == "Rust library attribution"
    assert not list(output.rglob("private.env"))
    assert not list(output.rglob("Cargo.toml"))
    assert json.loads((output / "dependencies.json").read_text())[0]["name"] == "ffi"
    assert hashlib.sha256(binary.read_bytes()).hexdigest() in (output / "SHA256SUMS").read_text()
    (output / "stale.txt").touch()
    stage_package(tmp_path, target, metadata, {"commit": "abc"}, tmp_path / "rust-notices")
    assert not (output / "stale.txt").exists()


def test_package_rejects_missing_dependency_notice(tmp_path):
    prepare_root(tmp_path, TARGETS[0])
    metadata = metadata_for(tmp_path)
    shutil.rmtree(tmp_path / "upstream")
    with pytest.raises(ValueError, match="No license/notice files found"):
        stage_package(tmp_path, TARGETS[0], metadata, {}, tmp_path / "rust-notices")


def test_package_rejects_path_outside_canonical_builds(tmp_path):
    with pytest.raises(ValueError, match="Unsupported distribution target"):
        stage_package(tmp_path, "../../../other", {}, {}, None)
