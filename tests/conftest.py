"""Recreate explicitly selected disposable pytest output after cleanup."""
from pathlib import Path


def pytest_configure(config):
    if config.option.basetemp:
        Path(config.option.basetemp).absolute().parent.mkdir(parents=True, exist_ok=True)
