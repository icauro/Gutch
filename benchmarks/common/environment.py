"""Benchmark-owned runtime environment loading from YAML configuration."""

import os
from pathlib import Path

from ..benchmark_config import DEFAULT_CONFIG_PATH, load_benchmark_config


DEFAULT_CONFIG_FILE = DEFAULT_CONFIG_PATH


def load_benchmark_environment(path: Path | None = None, *, overwrite: bool = False) -> Path:
    """Load benchmark YAML environment values without consulting repository config."""
    config = load_benchmark_config(path)
    for name, value in config.environment.items():
        if overwrite:
            os.environ[name] = value
        else:
            os.environ.setdefault(name, value)
    return config.path
