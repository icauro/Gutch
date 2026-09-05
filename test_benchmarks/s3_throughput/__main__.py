"""Interactive and explicit entry point for object-storage benchmarks."""

import argparse
from pathlib import Path
import sys
from typing import Sequence

if __package__ in {None, ""}:
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    __package__ = "gutch_repo.benchmarks.s3_throughput"

from .cli import add_arguments, run_command
from .config import StorageScenario, load_storage_scenario


FIXTURE_ROOT = Path(__file__).resolve().parent / "fixtures"


def _fixtures() -> list[StorageScenario]:
    return [load_storage_scenario(path) for path in sorted(FIXTURE_ROOT.glob("*.yaml"))]


def _print_fixtures() -> None:
    for scenario in _fixtures():
        print(f"storage/{scenario.name}: {scenario.description}")


def _prompt(label: str) -> str:
    try:
        return input(f"{label}: ").strip()
    except (EOFError, KeyboardInterrupt):
        return ""


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


def _optional_path(label: str) -> Path | None:
    value = _prompt(f"{label} (blank for automatic/default)")
    return Path(value).expanduser().resolve() if value else None


def _run_interactive() -> int:
    scenarios = _fixtures()
    if not scenarios:
        raise RuntimeError(f"No storage fixtures found in {FIXTURE_ROOT}")
    print("Storage benchmark interactive mode")
    choices = tuple(f"{scenario.name}: {scenario.description}" for scenario in scenarios)
    selected = _prompt_choice("Available storage fixtures:", choices)
    if selected is None:
        return 0
    scenario = scenarios[choices.index(selected)]
    workload = _prompt_choice("Storage action:", ("prepare", "download", "delete", "abort-multipart"))
    if workload is None:
        return 0
    target = _prompt("Target name (blank for all enabled targets)") or None
    work_dir = _optional_path("Work directory")
    output_dir = _optional_path("Download output directory") if workload == "download" else None
    prefix = None
    if workload == "delete":
        prefix = _prompt("Prefix (blank for scenario default)") or None
    elif workload == "abort-multipart":
        prefix = _prompt("Prefix (blank for whole bucket)") or None
    yes = True
    if workload in {"delete", "abort-multipart"}:
        yes = _prompt("Confirm destructive action [type YES]") == "YES"
        if not yes:
            return 0
    try:
        run_command(
            argparse.Namespace(
                fixture=scenario.name,
                action=workload,
                target=target,
                work_dir=work_dir,
                output_dir=output_dir,
                prefix=prefix,
                yes=yes,
                config=None,
            )
        )
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Benchmark failed: {error}")
        return 1
    return 0


def _run_explicit(arguments: list[str]) -> int:
    if arguments[0] == "list":
        if len(arguments) != 1:
            raise SystemExit("storage list does not accept additional arguments")
        _print_fixtures()
        return 0
    parser = argparse.ArgumentParser(prog="gutch_repo.benchmarks.s3_throughput")
    add_arguments(parser)
    return run_command(parser.parse_args(arguments))


def main(argv: Sequence[str] | None = None) -> int:
    arguments = list(sys.argv[1:] if argv is None else argv)
    return _run_interactive() if not arguments else _run_explicit(arguments)


if __name__ == "__main__":
    raise SystemExit(main())
