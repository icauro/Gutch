"""Interactive and explicit entry point for Gutch benchmark workflows."""

import argparse
from dataclasses import dataclass
from pathlib import Path
import sys
from typing import Sequence

if __package__ in {None, ""}:
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    __package__ = "tests.benchmarks.comparison"

from .cli import (
    add_arguments,
    add_reconstruction_storage_arguments,
    add_s3_cycle_arguments,
    run_command,
    run_reconstruction_storage_command,
    run_s3_cycle_command,
)
from .binary import resolve_gutch_executable
from .config import FixtureCatalog
from .s3_cycle import load_s3_cycle
from ..docker.runner import run_docker_benchmark
from ..scheduling.config import load_scenario
from ..benchmark_config import load_benchmark_config


PACKAGE_ROOT = Path(__file__).resolve().parent
REPO_ROOT = PACKAGE_ROOT.parents[2]
DEFAULT_PRODUCTS_ROOT = load_benchmark_config().product_root
S3_FIXTURE_PATH = PACKAGE_ROOT / "fixtures" / "15_gutch_s3_cold_publication_cycle.yaml"
RECONSTRUCTION_FIXTURE_PATH = PACKAGE_ROOT / "fixtures" / "14_gutch_reconstruction_storage_paths.yaml"


@dataclass(frozen=True)
class InteractiveFixture:
    category: str
    name: str
    description: str


def _print_fixtures() -> None:
    for fixture in _interactive_fixtures():
        print(f"{fixture.category}/{fixture.name}: {fixture.description}")


def _interactive_fixtures() -> list[InteractiveFixture]:
    entries = [
        InteractiveFixture("gutch", fixture.name, fixture.description)
        for fixture in FixtureCatalog().list()
    ]
    s3_fixture, _ = load_s3_cycle(S3_FIXTURE_PATH)
    entries.append(InteractiveFixture("s3-cycle", s3_fixture.name, s3_fixture.description))
    reconstruction = load_scenario(RECONSTRUCTION_FIXTURE_PATH)
    entries.append(
        InteractiveFixture(
            "reconstruction-storage",
            reconstruction.name,
            str(reconstruction.values.get("description", "")),
        )
    )
    return sorted(entries, key=lambda fixture: fixture.name)


def _prompt(label: str, default: str | None = None) -> str:
    suffix = f" [{default}]" if default else ""
    try:
        value = input(f"{label}{suffix}: ").strip()
    except (EOFError, KeyboardInterrupt):
        return ""
    return value or (default or "")


def _prompt_choice(label: str, options: Sequence[str]) -> str | None:
    print(label)
    for index, option in enumerate(options, start=1):
        print(f"  {index}. {option}")
    while True:
        value = _prompt("Choose a number (blank to cancel)")
        if not value:
            return None
        try:
            index = int(value)
        except ValueError:
            print("Please enter one of the displayed numbers.")
            continue
        if 1 <= index <= len(options):
            return options[index - 1]
        print("Please enter one of the displayed numbers.")


def _product_directories(products_root: Path) -> list[Path]:
    if not products_root.is_dir():
        return []
    return [
        path
        for path in sorted(products_root.iterdir(), key=lambda item: item.name.lower())
        if path.is_dir() and not path.name.startswith(".") and "." not in path.name
    ]


def _target_directories(product: Path) -> list[Path]:
    return [
        path
        for path in sorted(product.iterdir(), key=lambda item: item.name.lower())
        if path.is_dir() and not path.name.startswith(".")
    ]


def _prompt_product() -> Path | None:
    products_root = DEFAULT_PRODUCTS_ROOT.expanduser().resolve()
    if not products_root.is_dir():
        print(f"Product lookup root does not exist: {products_root}")
        return None
    products = _product_directories(products_root)
    if not products:
        print(f"No products found in {products_root}")
        return None
    choices = tuple(
        f"{product.name} ({len(_target_directories(product))} targets)"
        for product in products
    )
    selected = _prompt_choice("Available products:", choices)
    return products[choices.index(selected)] if selected is not None else None


def _ensure_gutch_executable(configured: Path | None = None) -> Path:
    explicit = configured.resolve() if configured is not None else None
    return resolve_gutch_executable(REPO_ROOT, explicit)


def _run_interactive_fixture(fixture: InteractiveFixture, executable: Path) -> int | None:
    product = _prompt_product()
    if product is None:
        return None
    artifacts_root = None
    keep_artifacts = False

    if fixture.category == "gutch":
        return run_command(
            argparse.Namespace(
                fixture=fixture.name,
                product=product,
                gutch_exe=executable,
                artifacts_root=artifacts_root,
                keep_artifacts=keep_artifacts,
            )
        )

    common = {
        "fixture": fixture.name,
        "product": product,
        "gutch_exe": executable,
        "artifacts_root": artifacts_root,
        "config": None,
        "s3_prefix": "GUTCH_S3",
        "bucket": None,
        "endpoint": None,
        "region": None,
        "access_key_id": None,
        "secret_access_key": None,
        "keep_artifacts": keep_artifacts,
    }
    if fixture.category == "reconstruction-storage":
        common.update({"transfer_concurrency": 4, "upload_concurrency": 8})
        return run_reconstruction_storage_command(argparse.Namespace(**common))
    common.update(
        {
            "prefix_root": None,
            "transfer_concurrency": None,
            "transfer_max_concurrency": None,
            "upload_concurrency": None,
        }
    )
    return run_s3_cycle_command(argparse.Namespace(**common))


def _run_interactive() -> int:
    try:
        executable = _ensure_gutch_executable()
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Benchmark failed: {error}")
        return 1
    print("Gutch benchmark interactive mode")
    fixtures = _interactive_fixtures()
    choices = tuple(f"{item.category}/{item.name}" for item in fixtures)
    selected = _prompt_choice("Available Gutch fixtures:", choices)
    if selected is None:
        return 0
    fixture = fixtures[choices.index(selected)]
    try:
        _run_interactive_fixture(fixture, executable)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Benchmark failed: {error}")
        return 1
    return 0


def _run_explicit(arguments: list[str]) -> int:
    if arguments[0] == "list":
        if len(arguments) != 1:
            raise SystemExit("gutch list does not accept additional arguments")
        _print_fixtures()
        return 0
    if arguments[0] == "reconstruction-storage":
        parser = argparse.ArgumentParser(prog="tests.benchmarks.comparison reconstruction-storage")
        add_reconstruction_storage_arguments(parser)
        parsed = parser.parse_args(arguments[1:])
        parsed.gutch_exe = _ensure_gutch_executable(parsed.gutch_exe)
        return run_reconstruction_storage_command(parsed)
    if arguments[0] == "s3-cycle":
        parser = argparse.ArgumentParser(prog="tests.benchmarks.comparison s3-cycle")
        add_s3_cycle_arguments(parser)
        parsed = parser.parse_args(arguments[1:])
        parsed.gutch_exe = _ensure_gutch_executable(parsed.gutch_exe)
        return run_s3_cycle_command(parsed)
    if arguments[0] == "docker":
        parser = argparse.ArgumentParser(
            prog="tests.benchmarks.comparison docker"
        )
        parser.add_argument("--fixture", default="15_gutch_s3_cold_publication_cycle")
        parser.add_argument("--config", type=Path)
        parser.add_argument("--artifacts-root", type=Path)
        parser.add_argument("--transfer-concurrency", type=int)
        parser.add_argument("--transfer-max-concurrency", type=int)
        parser.add_argument("--keep-artifacts", action="store_true")
        parsed = parser.parse_args(arguments[1:])
        try:
            run_docker_benchmark(
                fixture=parsed.fixture,
                config_path=parsed.config,
                artifacts_root=parsed.artifacts_root,
                transfer_concurrency=parsed.transfer_concurrency,
                transfer_max_concurrency=parsed.transfer_max_concurrency,
                keep_artifacts=parsed.keep_artifacts,
            )
        except KeyboardInterrupt:
            print("Docker benchmark cancelled.", file=sys.stderr)
            return 130
        except (OSError, RuntimeError, ValueError) as error:
            raise SystemExit(f"Docker benchmark failed: {error}") from error
        return 0
    parser = argparse.ArgumentParser(prog="tests.benchmarks.comparison")
    add_arguments(parser)
    parsed = parser.parse_args(arguments)
    parsed.gutch_exe = _ensure_gutch_executable(parsed.gutch_exe)
    return run_command(parsed)


def main(argv: Sequence[str] | None = None) -> int:
    arguments = list(sys.argv[1:] if argv is None else argv)
    return _run_interactive() if not arguments else _run_explicit(arguments)


if __name__ == "__main__":
    raise SystemExit(main())
