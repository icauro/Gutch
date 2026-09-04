"""CLI for the v2 S3 object-storage workload."""

import argparse
import json
from pathlib import Path

from gutch_repo.tests.benchmarks.common.environment import load_benchmark_environment
from gutch_repo.tests.benchmarks.benchmark_config import load_benchmark_config
from .config import load_storage_scenario
from .workflow import run_abort_multipart_uploads, run_delete, run_download, run_prepare


FIXTURE_ROOT = Path(__file__).resolve().parent / "fixtures"


def run_command(args: argparse.Namespace) -> int:
    config_path = getattr(args, "config", None)
    config = load_benchmark_config(config_path)
    environment_path = load_benchmark_environment(config_path)
    print(f"Using benchmark config: {environment_path}")
    scenario = load_storage_scenario(FIXTURE_ROOT / f"{args.fixture}.yaml")
    targets = [target for target in scenario.targets if target.enabled]
    if args.target:
        targets = [target for target in targets if target.name == args.target]
    if not targets:
        raise SystemExit("no enabled storage targets matched")
    work_dir = (args.work_dir or config.artifacts_root / "storage").resolve()
    delete_prefix = args.prefix if args.prefix is not None else scenario.prefix
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
        elif args.action == "download":
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
        elif args.action == "delete":
            if not args.yes:
                raise ValueError("delete requires --yes")
            results.append(
                {
                    "target": target.name,
                    "operation": "delete",
                    "prefix": delete_prefix,
                    "object_count": run_delete(target, prefix=delete_prefix),
                }
            )
        else:
            if not args.yes:
                raise ValueError("abort-multipart requires --yes")
            results.append(
                {
                    "target": target.name,
                    "operation": "abort-multipart",
                    "prefix": args.prefix,
                    "upload_count": run_abort_multipart_uploads(
                        target,
                        prefix=args.prefix,
                    ),
                }
            )
    output = work_dir / f"{scenario.name}-{args.action}.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(results, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(output)
    return 0


def add_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--fixture", default="s3_object_storage_8mib")
    parser.add_argument("--action", choices=("prepare", "download", "delete", "abort-multipart"), required=True)
    parser.add_argument("--target")
    parser.add_argument("--work-dir", type=Path)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--prefix")
    parser.add_argument("--yes", action="store_true", help="Confirm destructive delete or abort-multipart actions")
    parser.add_argument("--config", type=Path, help="Benchmark YAML config with paths, S3 credentials, and SSH targets")
