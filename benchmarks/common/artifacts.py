"""Run and retention layout for benchmark artifacts."""

from dataclasses import dataclass, field
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import time

from .filesystem import remove_tree


def safe_component(value: str) -> str:
    cleaned = re.sub(r"[^A-Za-z0-9_.-]+", "-", value).strip("-")
    return cleaned or "unnamed"


def create_run_dir(root: Path, source_name: str, fixture_name: str) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y.%m.%d")
    prefix = f"{stamp}-{safe_component(source_name)}-{safe_component(fixture_name)}"
    for sequence in range(1, 1000):
        candidate = root / f"{prefix}-{sequence:03d}"
        try:
            candidate.mkdir()
            return candidate
        except FileExistsError:
            continue
    raise RuntimeError(f"could not allocate run directory under {root}")


@dataclass
class LaneLayout:
    root: Path
    stats: Path
    traces: Path
    payload: Path
    _created_at: float = field(default_factory=time.perf_counter, repr=False)


@dataclass
class RunLayout:
    root: Path

    def lane(self, label: str) -> LaneLayout:
        lane_root = self.root / "lanes" / safe_component(label)
        stats = lane_root / "stats"
        traces = lane_root / "traces"
        payload = lane_root / "payload"
        for directory in (stats, traces, payload):
            directory.mkdir(parents=True, exist_ok=True)
        return LaneLayout(lane_root, stats, traces, payload)

    def write_results(self, results: list[dict]) -> None:
        target = self.root / "results.json"
        target.write_text(json.dumps(results, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    def remove_payloads(self) -> None:
        payload_root = self.root / "lanes"
        if not payload_root.exists():
            return
        for lane in payload_root.iterdir():
            if lane.is_dir():
                remove_tree(lane / "payload")
