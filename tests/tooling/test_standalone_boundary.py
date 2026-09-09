"""Standalone sources exclude host tooling and mutable build inputs."""
import ast
from pathlib import Path
from types import SimpleNamespace

from test_services.build_image import ROOT, source_files
from test_services.fixture import Services


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
    import test_services.fixture as fixture
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
