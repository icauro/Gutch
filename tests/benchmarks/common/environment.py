"""Benchmark-owned runtime environment loading."""

from pathlib import Path
import os
import re

from ..benchmark_config import load_benchmark_config


_ENV_NAME_PATTERN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
DEFAULT_ENV_FILE = load_benchmark_config().environment_file


def load_benchmark_environment(path: Path | None = None) -> Path | None:
    """Load the benchmark environment without consulting repository config."""
    environment_path = (path or DEFAULT_ENV_FILE).expanduser().resolve()
    if not environment_path.is_file():
        if path is not None:
            raise FileNotFoundError(f"benchmark environment file not found: {environment_path}")
        return None
    for line_number, raw_line in enumerate(
        environment_path.read_text(encoding="utf-8").splitlines(), start=1
    ):
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        if line.startswith("export "):
            line = line[7:].strip()
        name, value = (part.strip() for part in line.split("=", 1))
        if not _ENV_NAME_PATTERN.fullmatch(name):
            raise ValueError(f"invalid environment variable name at {environment_path}:{line_number}")
        if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
            value = value[1:-1]
        os.environ.setdefault(name, value)
    return environment_path
