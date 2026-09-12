"""Build the standalone Linux image from an explicit source inventory."""
import argparse
import shutil
import tempfile
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[1]
SOURCE_PATHS = (
    "Cargo.toml", "Cargo.lock", "pyproject.toml", "uv.lock", ".cargo", "gutch.py",
    "src", "gutch-cli/Cargo.toml", "gutch-cli/src", "tools",
    "tests/public_api.rs", "tests/installation_plan.rs", "tests/__init__.py",
    "tests/run.py", "tests/repository_suite.py", "tests/support", "tests/scenarios", "tests/fixtures",
    "tests/datasets", "tests/tooling", "tests/benchmarks",
)
EXCLUDED_PARTS = {"__pycache__", ".artifacts", ".builds", "bin", ".pytest_cache"}


def source_files(root=ROOT):
    for name in SOURCE_PATHS:
        path = root / name
        candidates = path.rglob("*") if path.is_dir() else [path]
        for candidate in candidates:
            relative = candidate.relative_to(root)
            if (candidate.is_file() and not EXCLUDED_PARTS.intersection(relative.parts)
                    and candidate.name != "config.yml"):
                yield candidate, relative


def build_image(tag):
    intermediate = ROOT / ".builds/intermediate"
    intermediate.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="linux-source-", dir=intermediate) as directory:
        context = Path(directory).resolve()
        assert context.is_relative_to(intermediate.resolve())
        total = 0
        for path, relative in source_files():
            destination = context / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, destination)
            total += path.stat().st_size
        print(f"Gutch Linux build context: {total / 1048576:.2f} MiB", flush=True)
        subprocess.run(["docker", "build", "-t", tag, "-f",
                        str(context / "tools/docker/Dockerfile"), str(context)], check=True)



if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", default="gutch-builder")
    build_image(parser.parse_args().tag)
