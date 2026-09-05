"""Filesystem measurements and deterministic directory comparisons."""

from dataclasses import dataclass
import hashlib
from pathlib import Path
import shutil
from typing import Iterator


@dataclass(frozen=True)
class TreeStats:
    file_count: int
    total_bytes: int
    largest_file_bytes: int


def scan_tree(root: Path) -> TreeStats:
    file_count = 0
    total_bytes = 0
    largest_file_bytes = 0
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        size = path.stat().st_size
        file_count += 1
        total_bytes += size
        largest_file_bytes = max(largest_file_bytes, size)
    return TreeStats(file_count, total_bytes, largest_file_bytes)


def remove_tree(path: Path) -> None:
    if not path.exists():
        return

    def make_writable(function, target, _error):
        target_path = Path(target)
        target_path.chmod(0o700)
        function(target)

    for child in path.rglob("*"):
        try:
            child.chmod(0o700)
        except OSError:
            pass
    try:
        path.chmod(0o700)
    except OSError:
        pass
    shutil.rmtree(path, onerror=make_writable)


def _relative_files(root: Path) -> Iterator[Path]:
    yield from sorted(path.relative_to(root) for path in root.rglob("*") if path.is_file())


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def compare_directories(expected: Path, actual: Path) -> list[str]:
    expected_files = set(_relative_files(expected))
    actual_files = set(_relative_files(actual))
    differences = [f"missing: {path}" for path in sorted(expected_files - actual_files)]
    differences.extend(f"unexpected: {path}" for path in sorted(actual_files - expected_files))
    for relative in sorted(expected_files & actual_files):
        if _sha256(expected / relative) != _sha256(actual / relative):
            differences.append(f"different: {relative}")
    return differences
