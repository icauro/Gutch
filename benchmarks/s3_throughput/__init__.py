"""S3 object-storage benchmark adapter."""

from .config import StorageScenario, StorageTarget, load_storage_scenario
from .workflow import run_download, run_prepare

__all__ = ["StorageScenario", "StorageTarget", "load_storage_scenario", "run_download", "run_prepare"]
