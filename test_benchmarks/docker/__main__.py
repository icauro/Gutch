"""Interactive entry point for container Gutch benchmarks."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import sys
from typing import Sequence

if __package__ in {None, ""}:
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
    __package__ = "test_benchmarks.docker"

from ..benchmark_config import load_benchmark_config
from ..common.config import load_scenario
from ..gutch_fixture_runner.config import load_fixture
from ..gutch_fixture_runner.s3_cycle import load_s3_cycle
from .runner import run_docker_benchmark


FIXTURE_ROOT = Path(__file__).resolve().parents[1] / "gutch_fixture_runner" / "fixtures"


@dataclass(frozen=True)
class ContainerFixture:
    runner: str
    name: str
    description: str
    transfer_concurrency: int | None = None
    transfer_max_concurrency: int | None = None
    snapshot_limit: int | None = None


def _fixtures() -> list[ContainerFixture]:
    fixtures: list[ContainerFixture] = []
    for path in sorted(FIXTURE_ROOT.glob("*.yaml")):
        values = load_scenario(path).values
        execution = values.get("execution", {})
        if not isinstance(execution, dict) or execution.get("default_host") != "container":
            continue
        try:
            fixture = load_fixture(path)
        except ValueError:
            continue
        if fixture.runner == "gutch-s3":
            _, options = load_s3_cycle(path)
        elif fixture.runner != "gutch-fixture":
            continue
        else:
            options = None
        fixtures.append(
            ContainerFixture(
                fixture.runner,
                fixture.name,
                fixture.description,
                options.transfer_concurrency if options else None,
                options.transfer_max_concurrency if options else None,
                fixture.snapshot_limit,
            )
        )
    return fixtures


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


def _run_interactive(config_path: Path | None = None) -> int:
    config = load_benchmark_config(config_path)
    fixtures = _fixtures()
    if not fixtures:
        raise RuntimeError(f"No container Gutch fixtures found in {FIXTURE_ROOT}")

    print("Container Gutch benchmark interactive mode")
    print(f"Using benchmark config: {config.path}")
    choices = tuple(f"{item.name}: {item.description}" for item in fixtures)
    selected = _prompt_choice("Available container fixtures:", choices)
    if selected is None:
        return 0
    fixture = fixtures[choices.index(selected)]
    diagnostic = False
    if fixture.runner == "gutch-s3":
        profile_choices = (
            f"Fixture defaults ({fixture.transfer_concurrency}/{fixture.transfer_max_concurrency})",
            "Single-worker diagnostic (1/1)",
        )
        profile = _prompt_choice("Transfer profile (workers/max-workers):", profile_choices)
        if profile is None:
            return 0
        diagnostic = profile == profile_choices[1]
    run_docker_benchmark(
        fixture=fixture.name,
        config_path=config.path,
        transfer_concurrency=1 if diagnostic else None,
        transfer_max_concurrency=1 if diagnostic else None,
    )
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m test_benchmarks.docker")
    parser.add_argument("--config", type=Path, help="Benchmark YAML config")
    args = parser.parse_args(list(sys.argv[1:] if argv is None else argv))
    try:
        return _run_interactive(args.config)
    except KeyboardInterrupt:
        print("Container benchmark cancelled.", file=sys.stderr)
        return 130
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Container benchmark failed: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
