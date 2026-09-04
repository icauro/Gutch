"""Validated configuration for object-storage workloads."""

from dataclasses import dataclass
from pathlib import Path
from typing import Any

from scripts_benchmark_v2.common.config import load_scenario


@dataclass(frozen=True)
class StorageTarget:
    name: str
    env_prefix: str
    endpoint: str | None
    region: str
    bucket: str | None
    enabled: bool


@dataclass(frozen=True)
class StorageScenario:
    name: str
    description: str
    targets: tuple[StorageTarget, ...]
    prefix: str
    size_bytes: int
    object_count: int
    upload_concurrency: int
    download_concurrency: int
    download_runs: int
    source_path: Path


def _size_bytes(value: Any) -> int:
    if isinstance(value, int):
        return value
    if not isinstance(value, str):
        raise ValueError("storage size must be an integer or MiB string")
    value = value.strip().lower()
    if value.endswith("mib"):
        return int(float(value[:-3]) * 1024 * 1024)
    if value.endswith("mb"):
        return int(float(value[:-2]) * 1000 * 1000)
    return int(value)


def load_storage_scenario(path: Path) -> StorageScenario:
    document = load_scenario(path)
    values = document.values
    raw_targets = values.get("targets", [])
    if not isinstance(raw_targets, list) or not raw_targets:
        raise ValueError(f"storage scenario requires targets: {path}")
    targets = []
    names: set[str] = set()
    for raw in raw_targets:
        if not isinstance(raw, dict) or not isinstance(raw.get("name"), str):
            raise ValueError(f"storage target must have a name: {path}")
        if raw["name"] in names:
            raise ValueError(f"duplicate storage target: {raw['name']}")
        enabled = raw.get("enabled", True)
        if not isinstance(enabled, bool):
            raise ValueError(f"storage target enabled must be boolean: {path}")
        names.add(raw["name"])
        targets.append(
            StorageTarget(
                name=raw["name"],
                env_prefix=str(raw.get("env_prefix", "S3_MAIN")),
                endpoint=str(raw["endpoint"]) if raw.get("endpoint") else None,
                region=str(raw.get("region", "auto")),
                bucket=str(raw["bucket"]) if raw.get("bucket") else None,
                enabled=enabled,
            )
        )
    prepare = values.get("prepare", {})
    download = values.get("download", {})
    if not isinstance(prepare, dict) or not isinstance(download, dict):
        raise ValueError(f"storage scenario prepare/download must be mappings: {path}")
    object_count = int(prepare.get("object_count", 16))
    upload_concurrency = int(prepare.get("upload_concurrency", 4))
    download_concurrency = int(prepare.get("download_concurrency", 16))
    download_runs = int(prepare.get("download_runs", download.get("default_args", {}).get("runs", 1)))
    if min(object_count, upload_concurrency, download_concurrency, download_runs) < 1:
        raise ValueError("storage workload counts must be positive")
    return StorageScenario(
        name=document.name,
        description=str(values.get("description", "")),
        targets=tuple(targets),
        prefix=str(prepare.get("prefix", "benchmarks/objects")).strip("/"),
        size_bytes=_size_bytes(prepare.get("size", "8MiB")),
        object_count=object_count,
        upload_concurrency=upload_concurrency,
        download_concurrency=download_concurrency,
        download_runs=download_runs,
        source_path=path,
    )
