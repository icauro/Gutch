"""Gutch fixture catalog with diagnostic capture modes."""

from pathlib import Path
from tests.benchmarks.scheduling.config import ScenarioCatalog
from tests.benchmarks.comparison.fixture_config import (
    load_fixture as load_lane_fixture,
)
from tests.benchmarks.comparison.models import FixtureSpec

FIXTURE_ROOT = Path(__file__).with_name("fixtures")


def load_fixture(path: Path) -> FixtureSpec:
    return load_lane_fixture(
        path, gutch_workloads=("individual", "batch", "product-bulk")
    )


class FixtureCatalog:
    def __init__(self, root: Path = FIXTURE_ROOT):
        self.root = root

    def list(self) -> list[FixtureSpec]:
        return [
            load_fixture(document.source_path)
            for document in ScenarioCatalog(self.root).list()
            if document.values.get("runner", "gutch-fixture") == "gutch-fixture"
        ]

    def resolve(self, name: str) -> FixtureSpec:
        document = ScenarioCatalog(self.root).resolve(name)
        if document.values.get("runner", "gutch-fixture") != "gutch-fixture":
            raise ValueError(f"fixture {name!r} is not a local Gutch fixture")
        return load_fixture(document.source_path)
