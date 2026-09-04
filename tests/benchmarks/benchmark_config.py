"""Benchmark-owned configuration independent of the repository config."""

from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml


BENCHMARK_ROOT = Path(__file__).resolve().parent
DEFAULT_CONFIG_PATH = BENCHMARK_ROOT / "config.yml"
EXAMPLE_CONFIG_PATH = BENCHMARK_ROOT / "config.yml.example"


@dataclass(frozen=True)
class SshTarget:
    name: str
    host: str
    port: int
    user: str
    host_alias: str
    identity_file: Path | None

    @property
    def destination(self) -> str:
        if self.host_alias:
            return self.host_alias
        if not self.host or not self.user:
            raise ValueError(f"SSH target {self.name!r} requires host and user")
        return f"{self.user}@{self.host}"


@dataclass(frozen=True)
class BenchmarkConfig:
    path: Path
    product_root: Path
    artifacts_root: Path
    environment_file: Path
    remote_target: str
    remote_workspace_root: str
    remote_fixtures_root: str
    remote_python: str
    ssh_targets: dict[str, SshTarget]

    def ssh_target(self, name: str | None = None) -> SshTarget:
        selected = name or self.remote_target
        try:
            return self.ssh_targets[selected]
        except KeyError as error:
            available = ", ".join(sorted(self.ssh_targets)) or "none"
            raise ValueError(
                f"unknown benchmark SSH target {selected!r}; available: {available}"
            ) from error


def _mapping(values: object, name: str, path: Path) -> dict[str, Any]:
    if not isinstance(values, dict):
        raise ValueError(f"benchmark config {name} must be a mapping: {path}")
    return values


def _required_string(values: dict[str, Any], name: str, path: Path) -> str:
    value = values.get(name)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"benchmark config {name} must be a non-empty string: {path}")
    return value.strip()


def _positive_int(values: dict[str, Any], name: str, path: Path, default: int) -> int:
    value = values.get(name, default)
    if isinstance(value, bool) or not isinstance(value, int) or value < 1:
        raise ValueError(f"benchmark config {name} must be a positive integer: {path}")
    return value


def _configured_path(values: dict[str, Any], name: str, path: Path) -> Path:
    value = Path(_required_string(values, name, path)).expanduser()
    return value.resolve() if value.is_absolute() else (path.parent / value).resolve()


def load_benchmark_config(path: Path | None = None) -> BenchmarkConfig:
    config_path = (path or DEFAULT_CONFIG_PATH).expanduser().resolve()
    if not config_path.is_file():
        if path is not None:
            raise FileNotFoundError(f"benchmark config not found: {config_path}")
        config_path = EXAMPLE_CONFIG_PATH.resolve()
    values = yaml.safe_load(config_path.read_text(encoding="utf-8")) or {}
    root = _mapping(values, "root", config_path)
    local = _mapping(root.get("local", {}), "local", config_path)
    remote = _mapping(root.get("remote", {}), "remote", config_path)
    raw_ssh = _mapping(root.get("ssh", {}), "ssh", config_path)
    ssh_targets: dict[str, SshTarget] = {}
    for name, raw in raw_ssh.items():
        if not isinstance(name, str) or not name.strip():
            raise ValueError(f"benchmark SSH target names must be non-empty: {config_path}")
        target = _mapping(raw, f"ssh.{name}", config_path)
        identity = target.get("identity_file")
        ssh_targets[name] = SshTarget(
            name=name,
            host=str(target.get("host", "")).strip(),
            port=_positive_int(target, "port", config_path, 22),
            user=str(target.get("user", "")).strip(),
            host_alias=str(target.get("host_alias", "")).strip(),
            identity_file=(Path(identity).expanduser().resolve() if identity else None),
        )
    return BenchmarkConfig(
        path=config_path,
        product_root=_configured_path(local, "product_root", config_path),
        artifacts_root=_configured_path(local, "artifacts_root", config_path),
        environment_file=_configured_path(local, "environment_file", config_path),
        remote_target=_required_string(remote, "default_target", config_path),
        remote_workspace_root=_required_string(remote, "workspace_root", config_path),
        remote_fixtures_root=_required_string(remote, "fixtures_root", config_path),
        remote_python=_required_string(remote, "python", config_path),
        ssh_targets=ssh_targets,
    )
