"""CLI for the v2 S3 object-storage workload."""

import argparse
import json
from pathlib import Path

from scripts_benchmark_v2.common.environment import load_benchmark_environment
from .config import load_storage_scenario
from .workflow import run_download, run_prepare


FIXTURE_ROOT = Path(__file__).resolve().parent / "fixtures"


def run_command(args: argparse.Namespace) -> int:
    environment_path = load_benchmark_environment(getattr(args, "env_file", None))
    if environment_path is not None:
        print(f"Using benchmark environment: {environment_path}")
    scenario = load_storage_scenario(FIXTURE_ROOT / f"{args.fixture}.yaml")
    targets = [target for target in scenario.targets if target.enabled]
    if args.target:
        targets = [target for target in targets if target.name == args.target]
    if not targets:
        raise SystemExit("no enabled storage targets matched")
    work_dir = (args.work_dir or Path("scripts_benchmark_v2/.artifacts/storage")).resolve()
    results = []
    for target in targets:
        if args.action == "prepare":
            results.append(
                run_prepare(
                    target,
                    prefix=scenario.prefix,
                    size_bytes=scenario.size_bytes,
                    object_count=scenario.object_count,
                    workers=scenario.upload_concurrency,
                    work_dir=work_dir / target.name,
                ).as_json()
            )
        else:
            results.extend(
                result.as_json()
                for result in run_download(
                    target,
                    prefix=scenario.prefix,
                    object_count=scenario.object_count,
                    workers=scenario.download_concurrency,
                    runs=scenario.download_runs,
                    output_dir=args.output_dir or work_dir / "results",
                )
            )
    output = work_dir / f"{scenario.name}-{args.action}.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(results, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(output)
    return 0


def add_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--fixture", default="s3_object_storage_8mib")
    parser.add_argument("--action", choices=("prepare", "download"), required=True)
    parser.add_argument("--target")
    parser.add_argument("--work-dir", type=Path)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--env-file", type=Path, help="Benchmark environment file (default: scripts_benchmark_v2/.env)")
