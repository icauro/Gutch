"""Internal Gutch fixtures extend the shared parser with diagnostic capture modes."""

from pathlib import Path
from comparison_benchmark_suite.scheduling.config import ScenarioCatalog
from comparison_benchmark_suite.tools.config import (
    load_fixture as load_comparison_fixture,
)
from comparison_benchmark_suite.tools.models import FixtureSpec

FIXTURE_ROOT = Path(__file__).with_name("fixtures")


def load_fixture(path: Path) -> FixtureSpec:
    return load_comparison_fixture(
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
