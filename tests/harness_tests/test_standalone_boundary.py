"""Standalone sources exclude host tooling and mutable build inputs."""

import re
import tomllib
from types import SimpleNamespace

from _tools.build_image import ROOT, source_files
from _tools.services import Services


def test_source_inventory_excludes_private_configuration_and_artifacts():
    files = list(source_files())
    assert files
    for path, relative in files:
        assert path.resolve().is_relative_to(ROOT)
        assert not {".artifacts", ".builds", ".venv", "bin"}.intersection(
            relative.parts
        )
        assert relative.name != "config.yml"


def test_network_reservation_retries_a_concurrent_overlap(monkeypatch):
    import _tools.services as fixture

    monkeypatch.setattr(fixture.subprocess, "check_output", lambda *args, **kwargs: "")
    attempts = []

    def reserve(command, **kwargs):
        attempts.append(command)
        return SimpleNamespace(
            returncode=1 if len(attempts) == 1 else 0,
            stderr="Pool overlaps with other one on this address space",
        )

    monkeypatch.setattr(fixture.subprocess, "run", reserve)
    services = Services()
    services.reserve_network()
    assert services.network_created
    assert attempts[0][4] == "10.241.0.0/24"
    assert attempts[1][4] == "10.241.1.0/24"
    assert attempts[0][-1] == attempts[1][-1] == services.project


def test_local_cargo_dependencies_stay_inside_the_project():
    for manifest in (ROOT / "Cargo.toml", ROOT / "gutch-cli/Cargo.toml"):
        configuration = tomllib.loads(manifest.read_text(encoding="utf-8"))
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            for dependency in configuration.get(section, {}).values():
                if isinstance(dependency, dict) and "path" in dependency:
                    assert (
                        (manifest.parent / dependency["path"])
                        .resolve()
                        .is_relative_to(ROOT)
                    )


def test_reconstruction_requires_explicit_storage(monkeypatch):
    import os
    from tests.benchmarks.scheduling.config import load_scenario
    from tests.benchmarks.comparison.cli import _s3_settings
    import pytest

    for name in list(os.environ):
        if name.startswith("GUTCH_S3_"):
            monkeypatch.delenv(name)
    fixture = load_scenario(
        ROOT
        / "tests/benchmarks/comparison/fixtures/14_gutch_reconstruction_storage_paths.yaml"
    )
    args = SimpleNamespace(
        bucket=None,
        endpoint=None,
        region=None,
        access_key_id=None,
        secret_access_key=None,
    )
    with pytest.raises(ValueError, match="GUTCH_S3_BUCKET_NAME"):
        _s3_settings(args, fixture.values["storage"])
    monkeypatch.setenv("GUTCH_S3_BUCKET_NAME", "custom-benchmark-bucket")
    monkeypatch.setenv("GUTCH_S3_ENDPOINT", "http://localhost:8333")
    settings = _s3_settings(args, fixture.values["storage"])
    assert settings.bucket == "custom-benchmark-bucket"
    assert settings.endpoint == "http://localhost:8333"


def production_source(source):
    """Remove test-gated items, retaining production code before and after them."""
    # Blank comments and strings for brace matching; keep offsets into the source.
    literals = re.compile(
        r'r(#+)".*?"\1|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])\'|//[^\n]*|/\*.*?\*/', re.S
    )
    masked = literals.sub(lambda match: " " * len(match[0]), source)
    ranges = []
    for attribute in re.finditer(r"#\[cfg\(test\)\]", masked):
        start = attribute.start()
        if ranges and start < ranges[-1][1]:
            continue
        item = attribute.end()
        # Test-only imports have a semicolon even when their use tree has braces.
        declaration = re.sub(r"^\s*(?:#\[[^\]]*\]\s*)*", "", masked[item:])
        if re.match(r"(?:pub(?:\([^)]*\))?\s+)?(?:use\b|mod\s+\w+\s*;)", declaration):
            end = masked.index(";", item) + 1
        else:
            opening = masked.index("{", item)
            depth = 1
            end = opening + 1
            while depth:
                depth += (masked[end] == "{") - (masked[end] == "}")
                end += 1
        ranges.append((start, end))
    for start, end in reversed(ranges):
        source = source[:start] + " " * (end - start) + source[end:]
    return source


def implementation_sources(directory):
    """Exclude dedicated tests and inline test modules from dependency checks."""
    for path in sorted((ROOT / "src" / directory).rglob("*.rs")):
        if "test" in path.stem:
            continue
        yield path, production_source(path.read_text(encoding="utf-8"))


def test_source_scanner_keeps_production_after_test_only_items():
    source = """#[cfg(test)]
use std::io::{Cursor, Read};
use crate::manifest::CompressionKind;
#[cfg(test)]
#[path = "tests.rs"]
mod external_tests;
#[cfg(test)]
mod tests { fn example() { let text = r#"}"#; } }
use crate::repo::coordination;
"""
    production = production_source(source)
    assert "CompressionKind" in production
    assert "crate::repo::coordination" in production
    assert "Cursor" not in production
    assert "fn example" not in production


def test_source_scanner_keeps_production_after_test_only_function():
    source = """impl Codec {
#[cfg(test)]
fn decode_test() { let text = "{"; }
fn encode() { crate::repo::coordination::write(); }
}"""
    production = production_source(source)
    assert "fn decode_test" not in production
    assert "crate::repo::coordination::write" in production


def test_repository_persistence_does_not_depend_on_capture_or_installation():
    for path, source in implementation_sources("repo"):
        assert not re.search(r"crate::(?:capture|installation)::", source), (
            path.relative_to(ROOT)
        )


def test_transfer_does_not_orchestrate_repository_or_installation_workflows():
    forbidden = re.compile(
        r"crate::(?:capture|installation)::|crate::repo::(?:coordination|paths|snapshot_manifests|fetch)::"
    )
    for path, source in implementation_sources("transfer"):
        # ArtifactRef is the existing descriptor shared with the repository;
        # acquiring locks, reading snapshot metadata and publication belong to workflows.
        assert not forbidden.search(source), path.relative_to(ROOT)


def test_artifact_codecs_do_not_depend_on_repository_workflows():
    for path, source in implementation_sources("artifact"):
        assert not re.search(
            r"crate::(?:repo|capture|installation|manifest)::", source
        ), path.relative_to(ROOT)


def test_public_api_only_reexports_owner_operations_and_contracts():
    source = (ROOT / "src/api.rs").read_text(encoding="utf-8")
    assert not re.search(r"pub(?:\([^)]*\))?\s+fn\s", source)
