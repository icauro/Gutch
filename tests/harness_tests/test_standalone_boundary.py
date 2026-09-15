"""Standalone sources exclude host tooling and mutable build inputs."""
import ast
import re
import tomllib
from pathlib import Path
from types import SimpleNamespace

from tools.build_image import ROOT, source_files
from tools.services import Services


def test_source_inventory_excludes_private_configuration_and_artifacts():
    files = list(source_files())
    assert files
    for path, relative in files:
        assert path.resolve().is_relative_to(ROOT)
        assert not {".artifacts", ".builds", ".venv", "bin"}.intersection(relative.parts)
        assert relative.name != "config.yml"
        if path.suffix != ".py":
            continue
        for node in ast.walk(ast.parse(path.read_text(encoding="utf-8"))):
            names = [node.module or ""] if isinstance(node, ast.ImportFrom) else (
                [alias.name for alias in node.names] if isinstance(node, ast.Import) else [])
            assert not any(name.startswith(("scripts.", "tests.common", "gutch_repo.")) for name in names), relative


def test_network_reservation_retries_a_concurrent_overlap(monkeypatch):
    import tools.services as fixture
    monkeypatch.setattr(fixture.subprocess, "check_output", lambda *args, **kwargs: "")
    attempts = []

    def reserve(command, **kwargs):
        attempts.append(command)
        return SimpleNamespace(returncode=1 if len(attempts) == 1 else 0,
                               stderr="Pool overlaps with other one on this address space")

    monkeypatch.setattr(fixture.subprocess, "run", reserve)
    services = Services()
    services.reserve_network()
    assert services.network_created
    assert attempts[0][4] == "10.241.0.0/24"
    assert attempts[1][4] == "10.241.1.0/24"
    assert attempts[0][-1] == attempts[1][-1] == services.project


def test_consumer_names_stay_in_the_integration_example():
    forbidden = re.compile(r"private-consumer-names", re.IGNORECASE)
    example = ROOT / "docs/current_state/hostapp_integration_example.md"
    candidates = {path for path, _ in source_files()}
    candidates.update((ROOT / "docs").rglob("*.md"))
    candidates.update(ROOT / name for name in ("README.md", "usage.md", "AGENTS.md", "index.md", "tests/benchmarks/README.md", "tests/benchmarks/.gitignore"))
    for path in sorted(candidates):
        if not path.is_file() or path == example or path.resolve() == Path(__file__).resolve():
            continue
        if path.suffix not in {".py", ".rs", ".md", ".toml", ".yml", ".yaml", ".example"} and path.name != ".gitignore":
            continue
        content = path.read_text(encoding="utf-8")
        if path.suffix == ".md":
            content = content.replace("hostapp_integration_example.md", "consumer_example.md")
        assert not forbidden.search(content), path.relative_to(ROOT)


def test_local_cargo_dependencies_stay_inside_the_project():
    for manifest in (ROOT / "Cargo.toml", ROOT / "gutch-cli/Cargo.toml"):
        configuration = tomllib.loads(manifest.read_text(encoding="utf-8"))
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            for dependency in configuration.get(section, {}).values():
                if isinstance(dependency, dict) and "path" in dependency:
                    assert (manifest.parent / dependency["path"]).resolve().is_relative_to(ROOT)


def test_reconstruction_requires_explicit_storage(monkeypatch):
    import os
    from tests.benchmarks.scheduling.config import load_scenario
    from tests.benchmarks.comparison.cli import _s3_settings
    import pytest
    for name in list(os.environ):
        if name.startswith("GUTCH_S3_"):
            monkeypatch.delenv(name)
    fixture = load_scenario(ROOT / "tests/benchmarks/comparison/fixtures/14_gutch_reconstruction_storage_paths.yaml")
    args = SimpleNamespace(bucket=None, endpoint=None, region=None, access_key_id=None, secret_access_key=None)
    with pytest.raises(ValueError, match="GUTCH_S3_BUCKET_NAME"):
        _s3_settings(args, fixture.values["storage"])
    monkeypatch.setenv("GUTCH_S3_BUCKET_NAME", "custom-benchmark-bucket")
    monkeypatch.setenv("GUTCH_S3_ENDPOINT", "http://localhost:8333")
    settings = _s3_settings(args, fixture.values["storage"])
    assert settings.bucket == "custom-benchmark-bucket"
    assert settings.endpoint == "http://localhost:8333"
