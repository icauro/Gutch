"""Benchmark-owned runtime environment loading."""

from pathlib import Path

from scripts._common.env import import_run_env_file


DEFAULT_ENV_FILE = Path(__file__).resolve().parents[1] / ".env"


def load_benchmark_environment(path: Path | None = None) -> Path | None:
    """Load the benchmark environment without consulting repository config."""
    environment_path = (path or DEFAULT_ENV_FILE).expanduser().resolve()
    if not environment_path.is_file():
        if path is not None:
            raise FileNotFoundError(f"benchmark environment file not found: {environment_path}")
        return None
    import_run_env_file(environment_path, overwrite=False)
    return environment_path
