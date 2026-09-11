#!/usr/bin/env python3
"""Gutch developer commands. Run --help to find setup, tests and benchmarks."""
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent


def main():
    python = ROOT / ".venv" / ("Scripts/python.exe" if sys.platform == "win32" else "bin/python")
    if python.is_file() and Path(sys.prefix).resolve() != (ROOT / ".venv").resolve():
        return subprocess.call([str(python), str(Path(__file__).resolve()), *sys.argv[1:]])
    from tools.runner import main as run
    return run(sys.argv[1:])


if __name__ == "__main__":
    raise SystemExit(main())
