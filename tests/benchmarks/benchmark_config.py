"""Benchmark-owned configuration independent of the repository config."""

from dataclasses import dataclass
from pathlib import Path
import re
from typing import Any

import yaml


_ENV_NAME_PATTERN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


BENCHMARK_ROOT = Path(__file__).resolve().parent
DEFAULT_CONFIG_PATH = BENCHMARK_ROOT / "config.yml"
EXAMPLE_CONFIG_PATH = BENCHMARK_ROOT / "config.yml.example"


@dataclass(frozen=True)
class BenchmarkConfig:
    path: Path
    product_root: Path
    artifacts_root: Path
    environment: dict[str, str]
    blender_versions: tuple[str, ...]
    blender_platforms: tuple[str, ...]


def _mapping(values: object, name: str, path: Path) -> dict[str, Any]:
    if not isinstance(values, dict):
        raise ValueError(f"benchmark config {name} must be a mapping: {path}")
    return values


def _required_string(values: dict[str, Any], name: str, path: Path) -> str:
    value = values.get(name)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"benchmark config {name} must be a non-empty string: {path}")
    return value.strip()


def _configured_path(values: dict[str, Any], name: str, path: Path) -> Path:
    value = Path(_required_string(values, name, path)).expanduser()
    return value.resolve() if value.is_absolute() else (path.parent / value).resolve()


def _environment(values: object, path: Path) -> dict[str, str]:
    raw_environment = _mapping(values, "environment", path)
    environment: dict[str, str] = {}
    for name, value in raw_environment.items():
        if not isinstance(name, str) or not _ENV_NAME_PATTERN.fullmatch(name):
            raise ValueError(f"benchmark environment names must be valid shell names: {path}")
        if not isinstance(value, str):
            raise ValueError(f"benchmark environment values must be strings: {path}")
        environment[name] = value
    return environment


def _string_tuple(values: object, name: str, path: Path, default: tuple[str, ...]) -> tuple[str, ...]:
    if values is None:
        return default
    if not isinstance(values, list) or not values or any(not isinstance(value, str) or not value.strip() for value in values):
        raise ValueError(f"benchmark config {name} must be a non-empty string list: {path}")
    return tuple(value.strip() for value in values)


def load_benchmark_config(path: Path | None = None) -> BenchmarkConfig:
    config_path = (path or DEFAULT_CONFIG_PATH).expanduser().resolve()
    if not config_path.is_file():
        if path is not None:
            raise FileNotFoundError(f"benchmark config not found: {config_path}")
        config_path = EXAMPLE_CONFIG_PATH.resolve()
    values = yaml.safe_load(config_path.read_text(encoding="utf-8")) or {}
    root = _mapping(values, "root", config_path)
    local = _mapping(root.get("local", {}), "local", config_path)
    environment = _environment(root.get("environment", {}), config_path)
    dataset = _mapping(root.get("dataset", {}), "dataset", config_path)
    return BenchmarkConfig(
        path=config_path,
        product_root=_configured_path(local, "product_root", config_path),
        artifacts_root=_configured_path(local, "artifacts_root", config_path),
        environment=environment,
        blender_versions=_string_tuple(
            dataset.get("blender_versions"),
            "dataset.blender_versions",
            config_path,
            ("5.2.0", "5.2.1"),
        ),
        blender_platforms=_string_tuple(
            dataset.get("blender_platforms"),
            "dataset.blender_platforms",
            config_path,
            ("linux-x64",),
        ),
    )
