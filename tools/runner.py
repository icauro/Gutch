"""Developer command routing; the engine CLI remains gutch-cli."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def run_module(module, arguments):
    return subprocess.call([sys.executable, "-m", module, *arguments], cwd=ROOT)


def replay(directory):
    from tests.run import SCENARIOS
    from tests.support.reproduction import source_revision
    path = directory.resolve() / "replay.json"
    record = json.loads(path.read_text(encoding="utf-8"))
    if record.get("schema_version") != 1 or record.get("case") not in {s.name for s in SCENARIOS}:
        raise ValueError(f"Unsupported replay record: {path}")
    current = source_revision()
    if record["revision"] != current:
        print(f"Source changed since this failure: {record['revision']} -> {current}", flush=True)
    print(f"Replaying {record['case']} in a new isolated run; original evidence is retained.", flush=True)
    return run_module("tests.run", ["--case", record["case"], "--keep-artifacts"])


def clean():
    # Only successful correctness payloads are eligible. Never remove failed runs,
    # compiler caches, arbitrary datasets or resources owned by another process.
    from tests.support.filesystem import remove_owned
    runs = ROOT / "tests/.artifacts/runs/gutch"
    removed = 0
    for manifest in runs.glob("*/run.json"):
        record = json.loads(manifest.read_text(encoding="utf-8"))
        if record.get("status") != "passed":
            continue
        for case in record.get("cases", []):
            owner = manifest.parent.resolve()
            if not owner.is_relative_to(runs.resolve()) or owner == runs.resolve():
                raise ValueError(f"Run escapes the artifact directory: {manifest}")
            case_root = (owner / case["evidence"]).resolve()
            if not case_root.is_relative_to(owner) or case_root == owner:
                raise ValueError(f"Case evidence escapes its run: {manifest}")
            payload = case_root / "payload"
            if payload.exists():
                remove_owned(payload, owner)
                removed += 1
    print(f"Removed {removed} successful case payloads. Reports and failure evidence retained.")
    return 0


def setup():
    if sys.version_info < (3, 12):
        raise ValueError("Python 3.12 or newer is required.")
    for command in ("uv", "cargo"):
        if not shutil.which(command):
            raise ValueError(f"Missing {command}. Install the prerequisites in README.md, then rerun setup.")
    subprocess.run(["uv", "sync", "--locked"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "--version"], cwd=ROOT, check=True)
    print("Ready. Run: python gutch.py test\nDocker is needed only for S3 tests and container benchmarks.")
    return 0


def main(arguments=None):
    arguments = list(sys.argv[1:] if arguments is None else arguments)
    parser = argparse.ArgumentParser(description="Develop Gutch: prepare, test, benchmark and reproduce failures.")
    commands = parser.add_subparsers(dest="command")
    commands.add_parser("setup", help="Prepare the locked Python environment; requires Python 3.12+, uv and Rust")
    commands.add_parser("test", add_help=False, help="Run tooling, Rust and local scenarios by default; --help lists suites")
    commands.add_parser("dataset", add_help=False, help="Generate deterministic datasets; --help lists seeds and recipes")
    commands.add_parser("benchmark", add_help=False, help="Run benchmarks; use 'list' to discover workloads")
    commands.add_parser("build-image", help="Build the standalone Linux image; requires Docker").add_argument("--tag", default="gutch-builder")
    commands.add_parser("replay", help="Rerun a case from a saved evidence directory").add_argument("directory", type=Path)
    commands.add_parser("clean", help="Remove retained successful test payloads, preserving reports and failures")
    routes = {"test": "tests.run", "dataset": "tests.datasets.generator.product", "benchmark": "tests.benchmarks.comparison"}
    if arguments and arguments[0] in routes:
        forwarded = arguments[1:]
        if arguments[0] == "benchmark" and forwarded[:1] == ["storage"]:
            return run_module("tests.benchmarks.s3_throughput", forwarded[1:])
        if arguments[0] == "benchmark" and forwarded == ["interactive"]:
            return run_module(routes["benchmark"], [])
        if arguments[0] == "benchmark" and forwarded == ["--help"]:
            print("Benchmark commands: list, interactive, storage, docker, s3-cycle, reconstruction-storage.\n"
                  "Use a command followed by --help for its options. Local comparison options follow:", flush=True)
        if arguments[0] == "benchmark" and not forwarded:
            forwarded = ["list"]
        return run_module(routes[arguments[0]], forwarded)
    args = parser.parse_args(arguments)
    try:
        if args.command == "setup":
            return setup()
        if args.command == "replay":
            return replay(args.directory)
        if args.command == "clean":
            return clean()
        if args.command == "build-image":
            from tools.build_image import build_image
            build_image(args.tag)
            return 0
        parser.print_help()
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Gutch: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
