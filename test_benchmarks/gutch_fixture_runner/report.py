"""Human-readable benchmark reports for completed Gutch lanes."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .models import FixtureSpec, LaneResult


def _format_mib(byte_count: int) -> str:
    return f"{byte_count / 1_048_576:.2f} MiB"


def _number(result: LaneResult, *keys: str) -> float | int | None:
    for key in keys:
        value = result.metrics.get(key)
        if isinstance(value, (int, float)):
            return value
    return None


def _seconds(result: LaneResult, *keys: str) -> str:
    value = _number(result, *keys)
    return "-" if value is None else f"{float(value):.2f} s"


def _speed(source_mib: float, result: LaneResult, *keys: str) -> str:
    value = _number(result, *keys)
    return "-" if value is None or float(value) <= 0 else f"{source_mib / float(value):.1f} MiB/s"


def _comparison(result: LaneResult) -> str:
    differences = result.metadata.get("differences", [])
    if not differences:
        return "OK"
    return f"FAIL ({len(differences)})" if isinstance(differences, list) else "FAIL"


def _table(title: str, headers: list[str], rows: list[tuple[str, list[str]]]) -> None:
    label_width = max(20, max(len(label) for label, _ in rows))
    lane_widths = [
        max(14, len(header), max(len(values[index]) for _, values in rows))
        for index, header in enumerate(headers)
    ]
    separator = "+" + "-" * (label_width + 2) + "+"
    for width in lane_widths:
        separator += "-" * (width + 2) + "+"
    print(separator)
    header_cells = [" " * label_width]
    header_cells.extend(header.ljust(width) for header, width in zip(headers, lane_widths, strict=True))
    print("| " + " | ".join(header_cells) + " |")
    print(separator)
    for label, values in rows:
        cells = [label.ljust(label_width)]
        cells.extend(value.rjust(width) for value, width in zip(values, lane_widths, strict=True))
        print("| " + " | ".join(cells) + " |")
    print(separator)


def print_comparison_table(
    fixture: FixtureSpec,
    product: Path,
    results: list[LaneResult],
) -> None:
    """Print the compact lane comparison table used by the legacy benchmark."""
    source_mib = max((float(result.metrics.get("input_bytes", 0)) for result in results), default=0) / 1_048_576

    def bytes_value(result: LaneResult, *keys: str) -> str:
        value = _number(result, *keys)
        return "-" if value is None else _format_mib(int(value))

    def ratio(result: LaneResult) -> str:
        value = _number(result, "repo_size", "growth")
        source_ratio = result.metrics.get("input_bytes", 0)
        return "-" if value is None or source_ratio == 0 else f"{float(value) / source_ratio * 100:.1f} %"

    rows = [
        ("Repo files", [str(int(_number(result, "repo_files") or 0)) for result in results]),
        ("Capture time", [_seconds(result, "backup") for result in results]),
        ("Capture min", [_seconds(result, "backup_min") for result in results]),
        ("Capture max", [_seconds(result, "backup_max") for result in results]),
        ("Reconstruct time", [_seconds(result, "restore", "fetch_reconstruct") for result in results]),
        ("Embedded verify", [_seconds(result, "verify") for result in results]),
        ("Corrupt time", [_seconds(result, "corrupt") for result in results]),
        ("Incremental repair", [_seconds(result, "repair") for result in results]),
        ("Repository check", [_seconds(result, "repository_check") for result in results]),
        ("Full restore recovery", [_seconds(result, "full_restore_recovery") for result in results]),
        ("Comparison", [_comparison(result) for result in results]),
        ("Repo growth", [bytes_value(result, "growth", "repo_size") for result in results]),
        ("Repo size ratio", [ratio(result) for result in results]),
        ("Repo size on disk", [bytes_value(result, "repo_size", "growth") for result in results]),
        ("Capture speed", [_speed(float(result.metrics.get("input_bytes", 0)) / 1_048_576, result, "backup") for result in results]),
        ("Reconstruct speed", [_speed(float(result.metrics.get("input_bytes", 0)) / 1_048_576, result, "restore", "fetch_reconstruct") for result in results]),
    ]
    print()
    print(f"  Fixture Benchmark: {fixture.name} / {product.name} (up to {source_mib:.1f} MiB selected per lane)")
    _table("Fixture Benchmark", [result.label for result in results], rows)


def _trace_paths(result: LaneResult) -> list[Path]:
    return [
        Path(path)
        for paths in result.traces.values()
        for path in paths
        if Path(path).is_file()
    ]


def _read_trace_spans(path: Path) -> list[tuple[str, float, dict[str, Any]]]:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return []
    events = document.get("traceEvents", [])
    if not isinstance(events, list):
        return []
    stacks: dict[tuple[object, object, str], list[tuple[int, dict[str, Any]]]] = {}
    spans: list[tuple[str, float, dict[str, Any]]] = []
    for event in events:
        if not isinstance(event, dict):
            continue
        name = event.get("name")
        phase = event.get("ph")
        timestamp = event.get("ts")
        if not isinstance(name, str) or not isinstance(timestamp, int) or name == "cli.command":
            continue
        key = (event.get("pid"), event.get("tid"), name)
        if phase == "B":
            args = event.get("args", {})
            stacks.setdefault(key, []).append((timestamp, args if isinstance(args, dict) else {}))
        elif phase == "E" and stacks.get(key):
            started_at, args = stacks[key].pop()
            spans.append((name, max(0, timestamp - started_at) / 1_000_000, args))
    return spans


def _aggregate_spans(paths: list[Path]) -> dict[str, tuple[int, float]]:
    aggregates: dict[str, tuple[int, float]] = {}
    for path in paths:
        for name, seconds, _ in _read_trace_spans(path):
            count, total = aggregates.get(name, (0, 0.0))
            aggregates[name] = (count + 1, total + seconds)
    return aggregates


def _aggregate_hash_metrics(paths: list[Path]) -> dict[str, Any]:
    count = 0
    mib = 0.0
    seconds = 0.0
    modes: dict[str, int] = {}
    workers: set[int] = set()
    for path in paths:
        for name, elapsed, args in _read_trace_spans(path):
            if name != "snapshot.reconstruct.file.full_hash":
                continue
            size_mib = args.get("size_mib")
            if not isinstance(size_mib, (int, float)):
                continue
            count += 1
            mib += float(size_mib)
            seconds += elapsed
            mode = args.get("hash_mode")
            if isinstance(mode, str):
                modes[mode] = modes.get(mode, 0) + 1
            configured_workers = args.get("configured_workers")
            if isinstance(configured_workers, int):
                workers.add(configured_workers)
    return {"count": count, "mib": mib, "seconds": seconds, "modes": modes, "workers": workers}


def _format_hash_modes(value: Any) -> str:
    return "-" if not isinstance(value, dict) or not value else ", ".join(
        f"{mode}={count}" for mode, count in sorted(value.items()) if isinstance(mode, str) and isinstance(count, int)
    )


def _format_workers(value: Any) -> str:
    return "-" if not isinstance(value, set) or not value else ", ".join(str(worker) for worker in sorted(value))


def print_gutch_profile_table(results: list[LaneResult]) -> None:
    """Print detailed Gutch trace spans and hash workload information."""
    aggregates = [(result.label, _aggregate_spans(_trace_paths(result))) for result in results]
    aggregates = [(label, values) for label, values in aggregates if values]
    if not aggregates:
        return
    names = sorted(
        set().union(*(values for _, values in aggregates)),
        key=lambda name: sum(values[name][1] for _, values in aggregates if name in values),
        reverse=True,
    )[:32]
    rows = [
        (
            name,
            ["-" if name not in values else f"{values[name][1]:.2f} s ({values[name][0]})" for _, values in aggregates],
        )
        for name in names
    ]
    hash_metrics = [(result.label, _aggregate_hash_metrics(_trace_paths(result))) for result in results]
    if any(int(metrics["count"]) > 0 for _, metrics in hash_metrics):
        by_label = {label: metrics for label, metrics in hash_metrics}
        rows.extend(
            [
                ("Hash files", [str(int(by_label[label]["count"])) for label, _ in aggregates]),
                ("Hash bytes", [f"{float(by_label[label]['mib']):.2f} MiB" for label, _ in aggregates]),
                ("Hash time", [f"{float(by_label[label]['seconds']):.2f} s" for label, _ in aggregates]),
                (
                    "Hash speed",
                    [
                        (
                            f"{float(by_label[label]['mib']) / float(by_label[label]['seconds']):.1f} MiB/s"
                            if float(by_label[label]["seconds"]) > 0
                            else "-"
                        )
                        for label, _ in aggregates
                    ],
                ),
                ("Hash modes", [_format_hash_modes(by_label[label]["modes"]) for label, _ in aggregates]),
                ("Reconstruct workers", [_format_workers(by_label[label]["workers"]) for label, _ in aggregates]),
            ]
        )
    print()
    print("  Gutch Perfetto Profiles")
    print("  note: values are summed completed spans; counts are in parentheses.")
    _table("Gutch Perfetto Profiles", [label for label, _ in aggregates], rows)


def print_fixture_report(fixture: FixtureSpec, product: Path, results: list[LaneResult]) -> None:
    print_comparison_table(fixture, product, results)
    print_gutch_profile_table(results)
