"""S3 object-storage benchmark adapter."""

from tests.benchmarks.s3_throughput.config import (
    StorageScenario,
    StorageTarget,
    load_storage_scenario,
)
from tests.benchmarks.s3_throughput.workflow import run_download, run_prepare

__all__ = [
    "StorageScenario",
    "StorageTarget",
    "load_storage_scenario",
    "run_download",
    "run_prepare",
]
