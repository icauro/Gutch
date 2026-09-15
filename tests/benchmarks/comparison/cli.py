"""Command-line entry points for the v2 Gutch adapter."""

import argparse
import json
import os
from pathlib import Path

from ..scheduling.environment import load_benchmark_environment
from ..scheduling.config import load_scenario
from ..benchmark_config import load_benchmark_config
from .binary import resolve_gutch_executable
from ..support.commands import GutchCli
from .config import FixtureCatalog, _wait_seconds, _profile
from .models import LaneResult
from .report import print_fixture_report
from ..support.reconstruction import run_reconstruction_storage
from ..support.s3 import S3Settings
from .s3_cycle import S3CycleOptions, load_s3_cycle, run_s3_cycle
from .workflow import run_fixture


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[3]


def _add_product_arguments(parser):
    from tests.datasets.generate import case_names
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--product", type=Path)
    selection.add_argument("--dataset", choices=case_names(),
                           help="Use a prepared corpus source without downloading or rebuilding repositories")


def _selected_product(args):
    if getattr(args, "dataset", None):
        from tests.datasets.generate import prepared_source
        return prepared_source(args.dataset)
    return args.product.resolve()


def add_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--fixture", default="gutch_default_trace_with_corrupt_repair")
    _add_product_arguments(parser)
    parser.add_argument("--gutch-exe", type=Path)
    parser.add_argument("--artifacts-root", type=Path)
    parser.add_argument("--config", type=Path, help="Benchmark YAML config (default: tests/benchmarks/config.yml)")
    parser.add_argument("--keep-artifacts", action="store_true")


def print_lane_result(result: LaneResult) -> None:
    """Print a completed lane while keeping the final JSON report machine-readable."""
    print()
    print(f"=== Benchmark lane: {result.label} ({result.kind}) ===")
    for name, value in result.metrics.items():
        print(f"  {name}: {value}")
    differences = result.metadata.get("differences", [])
    if differences:
        print("  differences:")
        for difference in differences:
            print(f"    {difference}")
    else:
        print("  differences: none")


def print_reconstruction_result(result: dict) -> None:
    print()
    print("=== Benchmark lane: s3-reconstruction (gutch-reconstruction-storage) ===")
    for name, value in result.get("timings", {}).items():
        print(f"  {name}: {value}")
    differences = result.get("differences", {})
    if differences:
        print("  differences:")
        for name, entries in differences.items():
            print(f"    {name}: {entries or 'none'}")
    else:
        print("  differences: none")


def run_command(args: argparse.Namespace) -> int:
    product = _selected_product(args)
    if not product.is_dir():
        raise SystemExit(f"product directory does not exist: {product}")
    repo_root = _repo_root()
    fixture = FixtureCatalog().resolve(args.fixture)
    executable = resolve_gutch_executable(repo_root, args.gutch_exe.resolve() if args.gutch_exe else None)
    artifacts = args.artifacts_root or load_benchmark_config(getattr(args, "config", None)).artifacts_root
    run_fixture(
        fixture,
        product,
        GutchCli(executable),
        artifacts,
        keep_artifacts=args.keep_artifacts,
        repo_root=repo_root,
        on_lane_complete=print_lane_result,
        on_run_complete=lambda results: print_fixture_report(fixture, product, results),
    )
    return 0


def add_reconstruction_storage_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--fixture", default="gutch_reconstruction_storage_paths")
    _add_product_arguments(parser)
    parser.add_argument("--gutch-exe", type=Path)
    parser.add_argument("--artifacts-root", type=Path)
    parser.add_argument("--config", type=Path, help="Benchmark YAML config with paths and S3 credentials")
    parser.add_argument("--s3-prefix")
    parser.add_argument("--bucket")
    parser.add_argument("--endpoint")
    parser.add_argument("--region")
    parser.add_argument("--access-key-id")
    parser.add_argument("--secret-access-key")
    parser.add_argument("--transfer-concurrency", type=int)
    parser.add_argument("--upload-concurrency", type=int)
    parser.add_argument("--keep-artifacts", action="store_true")


def run_reconstruction_storage_command(args: argparse.Namespace) -> int:
    product = _selected_product(args)
    if not product.is_dir():
        raise SystemExit(f"product directory does not exist: {product}")
    repo_root = _repo_root()
    executable = resolve_gutch_executable(repo_root, args.gutch_exe.resolve() if args.gutch_exe else None)
    fixture_path = Path(__file__).resolve().parent / "fixtures" / f"{args.fixture}.yaml"
    scenario = load_scenario(fixture_path)
    if scenario.values.get("runner") != "gutch-reconstruction-storage":
        raise SystemExit(f"fixture {args.fixture!r} is not a reconstruction-storage fixture")
    config_path = getattr(args, "config", None)
    config = load_benchmark_config(config_path)
    environment_path = load_benchmark_environment(config_path)
    print(f"Using benchmark config: {environment_path}")
    recovery_enabled = scenario.values.get("corrupt_repair", False)
    if not isinstance(recovery_enabled, bool):
        raise SystemExit("fixture corrupt_repair must be a boolean")
    open_perfetto = scenario.values.get("open_perfetto", False)
    if not isinstance(open_perfetto, bool):
        raise SystemExit("fixture open_perfetto must be a boolean")
    _wait_seconds(scenario.values, "lane_wait_seconds")
    step_wait_seconds = _wait_seconds(scenario.values, "step_wait_seconds")
    settings = _s3_settings(args, scenario.values.get("storage"))
    transfer_concurrency = args.transfer_concurrency if args.transfer_concurrency is not None else scenario.values.get('transfer', {}).get('concurrency', 4)
    upload_concurrency = args.upload_concurrency if args.upload_concurrency is not None else scenario.values.get('storage', {}).get('upload_concurrency', 8)
    if any(isinstance(value, bool) or not isinstance(value, int) or value < 1 for value in (transfer_concurrency, upload_concurrency)):
        raise SystemExit('concurrency values must be positive integers')
    result = run_reconstruction_storage(
        product,
        GutchCli(executable, profile_mode=scenario.values.get("profile_mode", "off")),
        settings,
        args.artifacts_root or config.artifacts_root,
        profile=_profile(scenario.values, scenario.name),
        transfer_concurrency=transfer_concurrency,
        upload_concurrency=upload_concurrency,
        recovery_enabled=recovery_enabled,
        open_perfetto=open_perfetto,
        step_wait_seconds=step_wait_seconds,
        keep_artifacts=args.keep_artifacts,
        on_run_complete=print_reconstruction_result,
    )
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


def _s3_settings(args: argparse.Namespace, storage: object = None) -> S3Settings:
    values = storage if isinstance(storage, dict) else {}
    env_prefix = getattr(args, "s3_prefix", None) or str(values.get("env_prefix", "GUTCH_S3")).strip() or "GUTCH_S3"
    access_key_env = str(values.get("access_key_env", "")).strip()
    secret_key_env = str(values.get("secret_key_env", "")).strip()

    def fixture_secret(environment_name: str, default_name: str) -> str:
        if environment_name:
            return os.environ.get(environment_name, "") or str(values.get(default_name, ""))
        return str(values.get(default_name, ""))

    defaults = {
        "bucket_name": str(values.get("bucket", "")),
        "endpoint": str(values.get("endpoint", "")),
        "region": str(values.get("region", "")),
        "access_key": fixture_secret(access_key_env, "access_key_default"),
        "secret_key": fixture_secret(secret_key_env, "secret_key_default"),
    }
    settings = S3Settings.from_environment(
        env_prefix,
        defaults=defaults,
        bucket_name=args.bucket or "",
        endpoint=args.endpoint or "",
        region=args.region or "",
        access_key=args.access_key_id or "",
        secret_key=args.secret_access_key or "",
    )
    if not settings.bucket:
        raise ValueError("S3 benchmarks require --bucket or GUTCH_S3_BUCKET_NAME in benchmark configuration")
    return settings


def add_s3_cycle_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--fixture", default="gutch_s3_cold_publication_cycle")
    _add_product_arguments(parser)
    parser.add_argument("--gutch-exe", type=Path)
    parser.add_argument("--artifacts-root", type=Path)
    parser.add_argument("--config", type=Path, help="Benchmark YAML config with paths and S3 credentials")
    parser.add_argument("--s3-prefix")
    parser.add_argument("--bucket")
    parser.add_argument("--endpoint")
    parser.add_argument("--region")
    parser.add_argument("--access-key-id")
    parser.add_argument("--secret-access-key")
    parser.add_argument("--prefix-root")
    parser.add_argument("--transfer-concurrency", type=int)
    parser.add_argument("--transfer-max-concurrency", type=int)
    parser.add_argument("--upload-concurrency", type=int)
    parser.add_argument("--keep-artifacts", action="store_true")


def run_s3_cycle_command(args: argparse.Namespace) -> int:
    product = _selected_product(args)
    if not product.is_dir():
        raise SystemExit(f"product directory does not exist: {product}")
    fixture_path = Path(__file__).resolve().parent / "fixtures" / f"{args.fixture}.yaml"
    fixture, defaults = load_s3_cycle(fixture_path)
    config_path = getattr(args, "config", None)
    config = load_benchmark_config(config_path)
    environment_path = load_benchmark_environment(config_path)
    print(f"Using benchmark config: {environment_path}")
    scenario_values = load_scenario(fixture_path).values
    options = S3CycleOptions(
        prefix_root=args.prefix_root or defaults.prefix_root,
        transfer_concurrency=args.transfer_concurrency if args.transfer_concurrency is not None else defaults.transfer_concurrency,
        transfer_max_concurrency=args.transfer_max_concurrency if args.transfer_max_concurrency is not None else defaults.transfer_max_concurrency,
        upload_concurrency=args.upload_concurrency if args.upload_concurrency is not None else defaults.upload_concurrency,
    )
    if min(options.transfer_concurrency, options.transfer_max_concurrency, options.upload_concurrency) < 1:
        raise SystemExit("concurrency values must be positive")
    if options.transfer_concurrency > options.transfer_max_concurrency:
        raise SystemExit("transfer concurrency exceeds transfer max concurrency")
    repo_root = _repo_root()
    executable = resolve_gutch_executable(repo_root, args.gutch_exe.resolve() if args.gutch_exe else None)
    result = run_s3_cycle(
        fixture,
        product,
        GutchCli(executable),
        _s3_settings(args, scenario_values.get("storage")),
        args.artifacts_root or config.artifacts_root,
        options=options,
        keep_artifacts=args.keep_artifacts,
        on_lane_complete=print_lane_result,
        on_run_complete=lambda results: print_fixture_report(fixture, product, results),
    )
    print(json.dumps([item.as_json() for item in result], indent=2, sort_keys=True))
    return 0
