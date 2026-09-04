"""Domain objects for Gutch fixture workflows."""

from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class GutchProfile:
    name: str
    chunk_profile: str = "chunk_128KiB"
    bundle_profile: str = "bundle_8MiB"
    compression_algorithm: str = "zstd"
    compression_level: int | None = 3
    storage_mode: str = "bundled"
    jobs: int | None = None


@dataclass(frozen=True)
class LaneSpec:
    label: str
    kind: str
    profile: GutchProfile
    warmup: bool = False
    arguments: tuple[str, ...] = ()


@dataclass(frozen=True)
class FixtureSpec:
    name: str
    runner: str
    description: str
    delete_artifacts: bool
    open_perfetto: bool
    lanes: tuple[LaneSpec, ...]
    source_path: Path
    corrupt_repair: bool = False
    lane_wait_seconds: float = 4.0
    step_wait_seconds: float = 4.0
    snapshot_limit: int | None = None


@dataclass
class LaneResult:
    label: str
    kind: str
    metrics: dict[str, float | int] = field(default_factory=dict)
    traces: dict[str, list[str]] = field(default_factory=dict)
    metadata: dict[str, Any] = field(default_factory=dict)

    def as_json(self) -> dict[str, Any]:
        return asdict(self)
