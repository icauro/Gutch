#!/usr/bin/env python3
# Shared deterministic and fuzz-driven dataset generation.
from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import shutil
import subprocess
import sys
import unicodedata
import unittest
from dataclasses import asdict, dataclass, field
from pathlib import Path

if __package__ in {None, ""}:
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
    __package__ = "tests.datasets"


METADATA_FILE_NAME = ".mock-product.json"
TEST_MANIFEST_FILE_NAME = ".gutch-test-manifest.json"
BASE_CONTENT_ROOT_NAME = ".mock-base"
SNAPSHOT_VARIANTS = [
    ("windows", "x86", "public"),
    ("windows", "x86", "supporter"),
]

_BINARY_EXTENSIONS = (".bin", ".dat", ".pak", ".tex")
_TEXT_EXTENSIONS = (".cfg", ".json", ".md", ".txt")
_PATH_WORDS = (
    "assets",
    "audio",
    "cache",
    "characters",
    "config",
    "content",
    "data",
    "levels",
    "locales",
    "materials",
    "models",
    "packs",
    "runtime",
    "scenes",
    "scripts",
    "shaders",
    "textures",
    "worlds",
)
_TEXT_BYTES = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 _-.,:;\n"
_TEXT_TRANSLATION = bytes(_TEXT_BYTES[index % len(_TEXT_BYTES)] for index in range(256))
_UNICODE_NAME_CHARACTERS = "áéíóúñçøåαβγδεζηабвгдеж東京大阪星空"
_MAX_SAFE_ABSOLUTE_PATH_LENGTH = 240
DEFAULT_SEED = 0
_TIMESTAMP_EPOCH_SECONDS = 1_700_000_000
_TIMESTAMP_EPOCH_SPAN_SECONDS = 5 * 365 * 24 * 60 * 60


TESTS_ROOT = Path(__file__).resolve().parents[1]
CORPUS_ROOT = TESTS_ROOT / "datasets/corpus"
TEST_DATASET_ROOT = TESTS_ROOT / "datasets"
TEST_DATASET_SOURCE_ROOT = TESTS_ROOT.parent / ".artifacts/tests/datasets/products"
DEFAULT_TEST_DATASET_ROOT = TEST_DATASET_SOURCE_ROOT


@dataclass(frozen=True)
class IntRange:
    minimum: int
    maximum: int

    def choose(self, rng: random.Random) -> int:
        return rng.randint(self.minimum, self.maximum)

    def capped_choose(self, rng: random.Random, maximum: int, label: str) -> int:
        capped_maximum = min(self.maximum, maximum)
        if self.minimum > capped_maximum:
            raise ValueError(
                f"{label} minimum {self.minimum} exceeds the available {maximum}"
            )
        return rng.randint(self.minimum, capped_maximum)

    def validate(self, label: str, *, allow_zero: bool = True) -> None:
        lowest = 0 if allow_zero else 1
        if self.minimum < lowest or self.maximum < self.minimum:
            raise ValueError(
                f"{label} range must satisfy {lowest} <= minimum <= maximum"
            )


@dataclass(frozen=True)
class GenerationRanges:
    product_size_bytes: IntRange
    file_count: IntRange
    path_depth: IntRange
    file_size_bytes: IntRange
    changed_files: IntRange
    patches_per_file: IntRange
    changed_bytes_per_file: IntRange
    added_files: IntRange
    removed_files: IntRange
    renamed_files: IntRange
    moved_files: IntRange
    long_path_files: IntRange
    relative_path_length: IntRange
    timestamp_age_seconds: IntRange
    readonly_files: IntRange
    executable_files: IntRange
    unicode_files: IntRange
    mixed_case_files: IntRange

    @classmethod
    def defaults_for_size_mb(cls, size_mb: int) -> GenerationRanges:
        total_bytes = size_mb * 1024 * 1024
        return cls(
            product_size_bytes=IntRange(total_bytes, total_bytes),
            file_count=IntRange(32, 64),
            path_depth=IntRange(1, 5),
            file_size_bytes=IntRange(256, total_bytes),
            changed_files=IntRange(1, 8),
            patches_per_file=IntRange(1, 4),
            changed_bytes_per_file=IntRange(1, min(64 * 1024, total_bytes)),
            added_files=IntRange(1, 5),
            removed_files=IntRange(1, 3),
            renamed_files=IntRange(0, 3),
            moved_files=IntRange(0, 3),
            long_path_files=IntRange(0, 3),
            relative_path_length=IntRange(120, 200),
            timestamp_age_seconds=IntRange(0, 365 * 24 * 60 * 60),
            readonly_files=IntRange(1, 3),
            executable_files=IntRange(1, 3),
            unicode_files=IntRange(1, 4),
            mixed_case_files=IntRange(1, 4),
        )

    def validate(self) -> None:
        self.product_size_bytes.validate("product size", allow_zero=False)
        self.file_count.validate("file count", allow_zero=False)
        self.path_depth.validate("path depth", allow_zero=True)
        self.file_size_bytes.validate("file size", allow_zero=False)
        self.changed_files.validate("changed files")
        self.patches_per_file.validate("patches per file", allow_zero=False)
        self.changed_bytes_per_file.validate("changed bytes per file", allow_zero=False)
        self.added_files.validate("added files")
        self.removed_files.validate("removed files")
        self.renamed_files.validate("renamed files")
        self.moved_files.validate("moved files")
        self.long_path_files.validate("long-path files")
        self.relative_path_length.validate("relative path length", allow_zero=False)
        self.timestamp_age_seconds.validate("timestamp age")
        self.readonly_files.validate("read-only files")
        self.executable_files.validate("executable files")
        self.unicode_files.validate("Unicode files")
        self.mixed_case_files.validate("mixed-case files")
        if self.path_depth.maximum > 8:
            raise ValueError(
                "path depth maximum must be <= 8 for cross-platform path safety"
            )
        if self.patches_per_file.minimum > self.changed_bytes_per_file.maximum:
            raise ValueError(
                "patches-per-file minimum cannot exceed changed-bytes maximum"
            )
        for label, file_range in (
            ("long-path files", self.long_path_files),
            ("read-only files", self.readonly_files),
            ("executable files", self.executable_files),
            ("Unicode files", self.unicode_files),
            ("mixed-case files", self.mixed_case_files),
        ):
            if file_range.minimum > self.file_count.maximum:
                raise ValueError(f"{label} minimum exceeds the maximum file count")

        feasible_counts = [
            count
            for count in range(self.file_count.minimum, self.file_count.maximum + 1)
            if max(
                self.product_size_bytes.minimum,
                count * self.file_size_bytes.minimum,
            )
            <= min(
                self.product_size_bytes.maximum,
                count * self.file_size_bytes.maximum,
            )
        ]
        if not feasible_counts:
            raise ValueError(
                "product size, file count, and file size ranges have no feasible combination"
            )
        minimum_required_files = (
            self.removed_files.minimum
            + self.changed_files.minimum
            + self.renamed_files.minimum
            + self.moved_files.minimum
        )
        if minimum_required_files > self.file_count.maximum:
            raise ValueError(
                "minimum per-version operations exceed the maximum file count"
            )

    def to_json(self) -> dict[str, list[int]]:
        return {
            field_name: [value.minimum, value.maximum]
            for field_name, value in self.__dict__.items()
        }

    @classmethod
    def from_json(cls, raw: dict[str, object]) -> GenerationRanges:
        product_bounds = raw.get("product_size_bytes")
        if not isinstance(product_bounds, list) or len(product_bounds) != 2:
            raise ValueError("fixture metadata is missing range product_size_bytes")
        default_size_mb = max(
            1, (int(product_bounds[1]) + 1024 * 1024 - 1) // (1024 * 1024)
        )
        defaults = cls.defaults_for_size_mb(default_size_mb)
        values: dict[str, IntRange] = {}
        for field_name in cls.__dataclass_fields__:
            bounds = raw.get(field_name)
            if isinstance(bounds, list) and len(bounds) == 2:
                values[field_name] = IntRange(int(bounds[0]), int(bounds[1]))
            else:
                values[field_name] = getattr(defaults, field_name)
        return cls(**values)


@dataclass
class _FileEntry:
    relative_path: str
    size_bytes: int
    seed: int
    is_binary: bool
    modified_at_ns: int = 0
    readonly: bool = False
    executable: bool = False


@dataclass(frozen=True)
class _BytePatch:
    relative_path: str
    offset: int
    replacement: bytes


@dataclass
class _VersionChanges:
    entries: list[_FileEntry]
    removed_paths: set[str]
    relocated_paths: dict[str, str]
    patches: list[_BytePatch]
    added_entries: list[_FileEntry]


@dataclass
class FixtureResult:
    fixture_dir: Path
    captured_snapshot_dirs: list[Path]
    created_version_labels: list[str]
    total_versions: int
    seed: int


def stable_hash(value: str) -> int:
    return int.from_bytes(hashlib.sha256(value.encode("utf-8")).digest()[:8], "big")


def _metadata_path(fixture_dir: Path) -> Path:
    return fixture_dir / METADATA_FILE_NAME


def _base_root(fixture_dir: Path) -> Path:
    return fixture_dir / BASE_CONTENT_ROOT_NAME


def _base_version_dir(fixture_dir: Path, version_label: str) -> Path:
    return _base_root(fixture_dir) / version_label


def _base_manifest_path(fixture_dir: Path, version_label: str) -> Path:
    return _base_root(fixture_dir) / f"{version_label}.entries.json"


def _version_label(version_index: int) -> str:
    return f"{version_index}.0.0"


def _snapshot_dir_name(
    version_label: str, platform: str, architecture: str, tier: str
) -> str:
    return f"{version_label}-{platform}-{architecture}-{tier}"


def _parse_snapshot_dir_name(path: Path) -> tuple[str, str, str, str] | None:
    parts = path.name.rsplit("-", 3)
    if len(parts) != 4:
        return None
    return parts[0], parts[1], parts[2], parts[3]


def _load_fixture_metadata(fixture_dir: Path) -> dict | None:
    path = _metadata_path(fixture_dir)
    if not path.is_file():
        return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return None


def _write_json(
    path: Path, value: object, *, modified_at_ns: int | None = None
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    if modified_at_ns is not None:
        os.utime(path, ns=(modified_at_ns, modified_at_ns))


def _save_fixture_metadata(
    fixture_dir: Path,
    metadata: dict,
    *,
    modified_at_ns: int | None = None,
) -> None:
    _write_json(_metadata_path(fixture_dir), metadata, modified_at_ns=modified_at_ns)


def _save_entries_manifest(
    fixture_dir: Path,
    version_label: str,
    entries: list[_FileEntry],
    *,
    modified_at_ns: int | None = None,
) -> None:
    manifest_path = _base_manifest_path(fixture_dir, version_label)
    _write_json(
        manifest_path,
        [asdict(entry) for entry in entries],
        modified_at_ns=modified_at_ns,
    )


def _load_entries_manifest(fixture_dir: Path, version_label: str) -> list[_FileEntry]:
    manifest_path = _base_manifest_path(fixture_dir, version_label)
    if not manifest_path.is_file():
        raise RuntimeError(
            f"Missing version manifest for {version_label}: {manifest_path}"
        )
    raw = json.loads(manifest_path.read_text(encoding="utf-8"))
    if not isinstance(raw, list):
        raise RuntimeError(f"Invalid version manifest shape for {version_label}")
    return [
        _FileEntry(
            relative_path=str(item["relative_path"]),
            size_bytes=int(item["size_bytes"]),
            seed=int(item["seed"]),
            is_binary=bool(item["is_binary"]),
            modified_at_ns=int(item.get("modified_at_ns", 0)),
            readonly=bool(item.get("readonly", False)),
            executable=bool(item.get("executable", False)),
        )
        for item in raw
    ]


def _detect_existing_version_count(fixture_dir: Path) -> int:
    highest_version = 0
    for entry in fixture_dir.iterdir():
        if not entry.is_dir() or entry.name.startswith("."):
            continue
        parsed = _parse_snapshot_dir_name(entry)
        if parsed is None:
            continue
        try:
            highest_version = max(highest_version, int(parsed[0].split(".", 1)[0]))
        except ValueError:
            continue
    return highest_version


def _random_token(rng: random.Random, length: int = 8) -> str:
    alphabet = "abcdefghijklmnopqrstuvwxyz0123456789"
    return "".join(rng.choice(alphabet) for _ in range(length))


def _path_key(relative_path: str) -> str:
    return unicodedata.normalize("NFC", relative_path).casefold()


def _has_unicode(relative_path: str) -> bool:
    return any(ord(character) > 127 for character in relative_path)


def _has_mixed_case(relative_path: str) -> bool:
    return any(character.islower() for character in relative_path) and any(
        character.isupper() for character in relative_path
    )


def _random_name(
    rng: random.Random,
    length: int,
    *,
    unicode_name: bool,
    mixed_case: bool,
) -> str:
    alphabet = "abcdefghijklmnopqrstuvwxyz0123456789"
    if mixed_case:
        alphabet += "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    if unicode_name:
        alphabet += _UNICODE_NAME_CHARACTERS
    characters = [rng.choice(alphabet) for _ in range(length)]
    if unicode_name:
        characters[0] = rng.choice(_UNICODE_NAME_CHARACTERS)
    if mixed_case and length >= 2:
        characters[-2] = rng.choice("ABCDEFGHIJKLMNOPQRSTUVWXYZ")
        characters[-1] = rng.choice("abcdefghijklmnopqrstuvwxyz")
    return unicodedata.normalize("NFC", "".join(characters))


def _random_relative_path(
    rng: random.Random,
    depth_range: IntRange,
    existing_paths: set[str],
    *,
    extension: str | None = None,
    filename: str | None = None,
    length_range: IntRange | None = None,
    unicode_name: bool = False,
    mixed_case: bool = False,
) -> str:
    existing_keys = {_path_key(path) for path in existing_paths}
    for _ in range(1_000):
        depth = depth_range.choose(rng)
        if filename is not None and (unicode_name or mixed_case) and depth == 0:
            continue
        resolved_extension = extension or rng.choice(
            _BINARY_EXTENSIONS + _TEXT_EXTENSIONS
        )
        if length_range is not None:
            desired_length = length_range.choose(rng)
            if filename is not None:
                directory_characters = desired_length - depth - len(filename)
                if depth == 0:
                    if directory_characters != 0:
                        continue
                    directories = []
                else:
                    if directory_characters < depth * 4:
                        continue
                    directory_lengths = _random_composition(
                        directory_characters, depth, 4, rng
                    )
                    directories = [
                        _random_name(
                            rng,
                            component_length,
                            unicode_name=unicode_name and index == 0,
                            mixed_case=mixed_case and index == 0,
                        )
                        for index, component_length in enumerate(directory_lengths)
                    ]
                resolved_filename = filename
            else:
                component_count = depth + 1
                name_characters = desired_length - depth - len(resolved_extension)
                if name_characters < component_count * 4:
                    continue
                component_lengths = _random_composition(
                    name_characters, component_count, 4, rng
                )
                directories = [
                    _random_name(
                        rng,
                        component_length,
                        unicode_name=unicode_name and index == 0,
                        mixed_case=mixed_case and index == 0,
                    )
                    for index, component_length in enumerate(component_lengths[:-1])
                ]
                resolved_filename = (
                    _random_name(
                        rng,
                        component_lengths[-1],
                        unicode_name=unicode_name and depth == 0,
                        mixed_case=mixed_case and depth == 0,
                    )
                    + resolved_extension
                )
        else:
            directories = [
                f"{rng.choice(_PATH_WORDS)}-{_random_token(rng, 5)}"
                for _ in range(depth)
            ]
            stem = _random_name(
                rng,
                12,
                unicode_name=unicode_name,
                mixed_case=mixed_case,
            )
            resolved_filename = filename or f"file-{stem}{resolved_extension}"
        relative_path = Path(*directories, resolved_filename).as_posix()
        if _path_key(relative_path) not in existing_keys:
            return relative_path
    raise RuntimeError("could not generate a unique randomized path")


def _safe_long_path_range(
    fixture_dir: Path,
    ranges: GenerationRanges,
    target_total_versions: int,
) -> IntRange | None:
    longest_version = _version_label(target_total_versions)
    longest_target_name = max(
        (
            _snapshot_dir_name(longest_version, platform, architecture, tier)
            for platform, architecture, tier in SNAPSHOT_VARIANTS
        ),
        key=len,
    )
    available = (
        _MAX_SAFE_ABSOLUTE_PATH_LENGTH - len(str(fixture_dir / longest_target_name)) - 1
    )
    maximum = min(ranges.relative_path_length.maximum, available)
    if ranges.long_path_files.minimum and maximum < ranges.relative_path_length.minimum:
        raise ValueError(
            "configured fixture root is too long for the requested long relative paths"
        )
    if maximum < ranges.relative_path_length.minimum:
        return None
    return IntRange(ranges.relative_path_length.minimum, maximum)


def _allocate_sizes(
    total_bytes: int, count: int, size_range: IntRange, rng: random.Random
) -> list[int]:
    minimum_total = count * size_range.minimum
    maximum_total = count * size_range.maximum
    if not minimum_total <= total_bytes <= maximum_total:
        raise ValueError(
            "selected total size cannot be allocated across the selected file count"
        )
    sizes = [size_range.minimum] * count
    remaining = total_bytes - minimum_total
    available_indexes = list(range(count))
    while remaining > 0 and available_indexes:
        index = rng.choice(available_indexes)
        capacity = size_range.maximum - sizes[index]
        increment = rng.randint(1, min(capacity, remaining)) if capacity else 0
        sizes[index] += increment
        remaining -= increment
        if sizes[index] == size_range.maximum:
            available_indexes.remove(index)
    if remaining:
        raise RuntimeError("failed to allocate the selected product size")
    rng.shuffle(sizes)
    return sizes


def _initial_entries(
    ranges: GenerationRanges,
    rng: random.Random,
    long_path_range: IntRange | None,
) -> list[_FileEntry]:
    feasible_counts = [
        count
        for count in range(ranges.file_count.minimum, ranges.file_count.maximum + 1)
        if max(
            ranges.product_size_bytes.minimum, count * ranges.file_size_bytes.minimum
        )
        <= min(
            ranges.product_size_bytes.maximum, count * ranges.file_size_bytes.maximum
        )
    ]
    file_count = rng.choice(feasible_counts)
    minimum_total = max(
        ranges.product_size_bytes.minimum,
        file_count * ranges.file_size_bytes.minimum,
    )
    maximum_total = min(
        ranges.product_size_bytes.maximum,
        file_count * ranges.file_size_bytes.maximum,
    )
    total_bytes = rng.randint(minimum_total, maximum_total)
    sizes = _allocate_sizes(total_bytes, file_count, ranges.file_size_bytes, rng)
    indexes = list(range(file_count))
    long_path_count = (
        ranges.long_path_files.capped_choose(rng, file_count, "long-path files")
        if long_path_range is not None
        else 0
    )
    long_path_indexes = set(rng.sample(indexes, long_path_count))
    unicode_indexes = set(
        rng.sample(
            indexes,
            ranges.unicode_files.capped_choose(rng, file_count, "Unicode files"),
        )
    )
    mixed_case_indexes = set(
        rng.sample(
            indexes,
            ranges.mixed_case_files.capped_choose(rng, file_count, "mixed-case files"),
        )
    )
    paths: set[str] = set()
    entries: list[_FileEntry] = []
    for index, size_bytes in enumerate(sizes):
        relative_path = _random_relative_path(
            rng,
            ranges.path_depth,
            paths,
            length_range=long_path_range if index in long_path_indexes else None,
            unicode_name=index in unicode_indexes,
            mixed_case=index in mixed_case_indexes,
        )
        paths.add(relative_path)
        entries.append(
            _FileEntry(
                relative_path=relative_path,
                size_bytes=size_bytes,
                seed=rng.getrandbits(63),
                is_binary=Path(relative_path).suffix.lower() in _BINARY_EXTENSIONS,
            )
        )
    return entries


def _deterministic_bytes(size: int, seed: int, *, text: bool) -> bytes:
    rng = random.Random(seed)
    generated = rng.randbytes(size)
    return generated.translate(_TEXT_TRANSLATION) if text else generated


def _write_entry(path: Path, entry: _FileEntry) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    rng = random.Random(entry.seed)
    remaining = entry.size_bytes
    with path.open("wb") as handle:
        while remaining:
            chunk = rng.randbytes(min(64 * 1024, remaining))
            if not entry.is_binary:
                chunk = chunk.translate(_TEXT_TRANSLATION)
            handle.write(chunk)
            remaining -= len(chunk)


def _timestamp_base_seconds(seed: int) -> int:
    return _TIMESTAMP_EPOCH_SECONDS + seed % _TIMESTAMP_EPOCH_SPAN_SECONDS


def _deterministic_timestamp_ns(timestamp_base_seconds: int, label: str) -> int:
    return timestamp_base_seconds * 1_000_000_000 + stable_hash(label) % 1_000_000_000


def _normalize_directory_timestamps(root: Path, modified_at_ns: int) -> None:
    directories = sorted(
        (path for path in root.rglob("*") if path.is_dir()),
        key=lambda path: len(path.parts),
        reverse=True,
    )
    for directory in directories + [root]:
        os.utime(directory, ns=(modified_at_ns, modified_at_ns))


def _assign_entry_metadata(
    entries: list[_FileEntry],
    ranges: GenerationRanges,
    rng: random.Random,
    timestamp_base_seconds: int,
) -> list[_FileEntry]:
    indexes = list(range(len(entries)))
    readonly_indexes = set(
        rng.sample(
            indexes,
            ranges.readonly_files.capped_choose(rng, len(entries), "read-only files"),
        )
    )
    executable_indexes = set(
        rng.sample(
            indexes,
            ranges.executable_files.capped_choose(
                rng, len(entries), "executable files"
            ),
        )
    )
    assigned: list[_FileEntry] = []
    for index, entry in enumerate(entries):
        age_seconds = ranges.timestamp_age_seconds.choose(rng)
        assigned.append(
            _FileEntry(
                relative_path=entry.relative_path,
                size_bytes=entry.size_bytes,
                seed=entry.seed,
                is_binary=entry.is_binary,
                modified_at_ns=(timestamp_base_seconds - age_seconds) * 1_000_000_000
                + rng.randrange(1_000_000_000),
                readonly=index in readonly_indexes,
                executable=index in executable_indexes,
            )
        )
    return assigned


def _apply_entry_metadata(base_dir: Path, entries: list[_FileEntry]) -> None:
    for entry in entries:
        path = base_dir / entry.relative_path
        if entry.modified_at_ns:
            os.utime(path, ns=(entry.modified_at_ns, entry.modified_at_ns))
        mode = 0o444 if entry.readonly else 0o644
        if entry.executable:
            mode |= 0o111
        path.chmod(mode)


def _make_tree_writable(root: Path) -> None:
    if not root.exists():
        return
    for path in root.rglob("*"):
        try:
            path.chmod(0o777 if path.is_dir() else 0o666)
        except OSError:
            pass


def _remove_tree(root: Path) -> None:
    if not root.exists():
        return
    _make_tree_writable(root)
    shutil.rmtree(root)


def _install_launch_test_executable(base_dir: Path, *, modified_at_ns: int) -> None:
    if os.name == "nt":
        source = (
            Path(os.environ.get("WINDIR", r"C:\Windows")) / "System32" / "where.exe"
        )
        destination = base_dir / "LaunchTest.exe"
    else:
        source = Path("/bin/true")
        destination = base_dir / "LaunchTest"
    if not source.is_file():
        raise RuntimeError(f"Launch test executable is unavailable: {source}")
    shutil.copy2(source, destination)
    os.utime(destination, ns=(modified_at_ns, modified_at_ns))


def _copy_tree_with_file_links(source_dir: Path, destination_dir: Path) -> None:
    destination_dir.mkdir(parents=True, exist_ok=True)
    for source_path in source_dir.rglob("*"):
        destination_path = destination_dir / source_path.relative_to(source_dir)
        if source_path.is_dir():
            destination_path.mkdir(parents=True, exist_ok=True)
            continue
        destination_path.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.link(source_path, destination_path)
        except OSError:
            shutil.copy2(source_path, destination_path)


def _materialize_tier_snapshot(
    base_dir: Path,
    fixture_dir: Path,
    version_label: str,
    platform: str,
    architecture: str,
    tier: str,
    timestamp_base_seconds: int,
) -> Path:
    snapshot_dir = fixture_dir / _snapshot_dir_name(
        version_label, platform, architecture, tier
    )
    _remove_tree(snapshot_dir)
    _copy_tree_with_file_links(base_dir, snapshot_dir)
    access_info_path = snapshot_dir / "metadata" / "access.json"
    access_info_path.parent.mkdir(parents=True, exist_ok=True)
    _write_json(
        access_info_path,
        {
            "version": version_label,
            "platform": platform,
            "architecture": architecture,
            "access": tier,
        },
        modified_at_ns=_deterministic_timestamp_ns(
            timestamp_base_seconds,
            f"{version_label}:{platform}:{architecture}:{tier}:access",
        ),
    )
    if tier == "supporter":
        bonus_path = snapshot_dir / "rewards" / "supporter" / "bonus.txt"
        bonus_path.parent.mkdir(parents=True, exist_ok=True)
        bonus_path.write_text(
            f"Supporter reward for {version_label} on {platform}/{architecture}\n",
            encoding="utf-8",
        )
        bonus_timestamp_ns = _deterministic_timestamp_ns(
            timestamp_base_seconds,
            f"{version_label}:{platform}:{architecture}:{tier}:bonus",
        )
        os.utime(bonus_path, ns=(bonus_timestamp_ns, bonus_timestamp_ns))
    return snapshot_dir


def _blake3_file_hex(path: Path) -> str:
    executable = shutil.which("b3sum")
    if executable is None:
        raise RuntimeError("--emit-test-manifest requires the b3sum executable on PATH")
    result = subprocess.run(
        [executable, "--no-names", str(path)],
        capture_output=True,
        check=True,
        text=True,
    )
    return result.stdout.strip()


def _write_test_manifest(
    fixture_dir: Path,
    *,
    seed: int,
    ranges: GenerationRanges,
    timestamp_base_seconds: int,
) -> None:
    snapshots: list[dict[str, object]] = []
    for snapshot_dir in sorted(path for path in fixture_dir.iterdir() if path.is_dir()):
        parsed_snapshot = _parse_snapshot_dir_name(snapshot_dir)
        if parsed_snapshot is None:
            continue
        version_label, platform, architecture, tier = parsed_snapshot
        entries = []
        for path in sorted(snapshot_dir.rglob("*")):
            relative_path = path.relative_to(snapshot_dir).as_posix()
            if path.is_symlink():
                raw_target = Path(os.readlink(path))
                target_path = (
                    (path.parent / raw_target)
                    .resolve(strict=False)
                    .relative_to(snapshot_dir.resolve())
                    .as_posix()
                )
                entries.append(
                    {
                        "entry_type": "symlink",
                        "path": relative_path,
                        "target_path": target_path,
                    }
                )
            elif path.is_dir():
                entries.append(
                    {
                        "entry_type": "directory",
                        "path": relative_path,
                        "read_only": not bool(path.stat().st_mode & 0o222),
                    }
                )
            elif path.is_file():
                mode = path.stat().st_mode
                entries.append(
                    {
                        "entry_type": "file",
                        "path": path.relative_to(snapshot_dir).as_posix(),
                        "size_bytes": path.stat().st_size,
                        "blake3": _blake3_file_hex(path),
                        "executable": os.name != "nt" and bool(mode & 0o111),
                        "read_only": not bool(mode & 0o222),
                    }
                )
        snapshots.append(
            {
                "path": snapshot_dir.name,
                "version": version_label,
                "platform": platform,
                "architecture": architecture,
                "tier": tier,
                "entries": entries,
            }
        )
    _write_json(
        fixture_dir / TEST_MANIFEST_FILE_NAME,
        {
            "schema_version": 1,
            "generator": {
                "seed": seed,
                "ranges": ranges.to_json(),
            },
            "snapshots": snapshots,
        },
        modified_at_ns=_deterministic_timestamp_ns(
            timestamp_base_seconds,
            "test-manifest",
        ),
    )


def _build_initial_version(
    base_dir: Path,
    entries: list[_FileEntry],
    *,
    launch_modified_at_ns: int,
) -> None:
    _remove_tree(base_dir)
    base_dir.mkdir(parents=True, exist_ok=True)
    for entry in entries:
        _write_entry(base_dir / entry.relative_path, entry)
    _install_launch_test_executable(base_dir, modified_at_ns=launch_modified_at_ns)


def _choose_add_remove_plan(
    previous_entries: list[_FileEntry],
    ranges: GenerationRanges,
    rng: random.Random,
    long_path_range: IntRange | None,
) -> tuple[set[str], int, int]:
    plans = [
        (removed_count, added_count)
        for removed_count in range(
            ranges.removed_files.minimum, ranges.removed_files.maximum + 1
        )
        for added_count in range(
            ranges.added_files.minimum, ranges.added_files.maximum + 1
        )
        if removed_count <= len(previous_entries)
        and len(previous_entries) - removed_count
        >= ranges.changed_files.minimum
        + ranges.renamed_files.minimum
        + ranges.moved_files.minimum
        and ranges.file_count.minimum
        <= len(previous_entries) - removed_count + added_count
        <= ranges.file_count.maximum
    ]
    rng.shuffle(plans)
    for removed_count, added_count in plans:
        for _ in range(100):
            removed = set(
                rng.sample(
                    [entry.relative_path for entry in previous_entries], removed_count
                )
            )
            retained_entries = [
                entry
                for entry in previous_entries
                if entry.relative_path not in removed
            ]
            minimum_patchable_size = max(
                ranges.patches_per_file.minimum,
                ranges.changed_bytes_per_file.minimum,
            )
            if (
                sum(
                    entry.size_bytes >= minimum_patchable_size
                    for entry in retained_entries
                )
                < ranges.changed_files.minimum
            ):
                continue
            retained_paths = [entry.relative_path for entry in retained_entries]
            feature_deficits = [
                max(
                    0,
                    ranges.unicode_files.minimum
                    - sum(_has_unicode(path) for path in retained_paths),
                ),
                max(
                    0,
                    ranges.mixed_case_files.minimum
                    - sum(_has_mixed_case(path) for path in retained_paths),
                ),
            ]
            if long_path_range is not None:
                feature_deficits.append(
                    max(
                        0,
                        ranges.long_path_files.minimum
                        - sum(
                            len(path) >= long_path_range.minimum
                            for path in retained_paths
                        ),
                    )
                )
            if max(feature_deficits, default=0) > added_count:
                continue
            retained_size = sum(entry.size_bytes for entry in retained_entries)
            minimum_total = max(
                ranges.product_size_bytes.minimum,
                retained_size + added_count * ranges.file_size_bytes.minimum,
            )
            maximum_total = min(
                ranges.product_size_bytes.maximum,
                retained_size + added_count * ranges.file_size_bytes.maximum,
            )
            if minimum_total <= maximum_total:
                return removed, added_count, rng.randint(minimum_total, maximum_total)
    raise ValueError(
        "operation and size ranges cannot produce the next version; widen added-file or file-size ranges"
    )


def _random_composition(
    total: int, parts: int, minimum_each: int, rng: random.Random
) -> list[int]:
    remaining = total - parts * minimum_each
    if parts == 1:
        return [total]
    separators = sorted(rng.sample(range(remaining + parts - 1), parts - 1))
    values: list[int] = []
    previous = -1
    for separator in separators + [remaining + parts - 1]:
        values.append(separator - previous - 1 + minimum_each)
        previous = separator
    return values


def _patches_for_entry(
    entry: _FileEntry, ranges: GenerationRanges, rng: random.Random
) -> list[_BytePatch]:
    patch_count = ranges.patches_per_file.capped_choose(
        rng,
        min(entry.size_bytes, ranges.changed_bytes_per_file.maximum),
        "patches per file",
    )
    minimum_changed = max(ranges.changed_bytes_per_file.minimum, patch_count)
    maximum_changed = min(ranges.changed_bytes_per_file.maximum, entry.size_bytes)
    if minimum_changed > maximum_changed:
        raise ValueError(
            f"changed-byte minimum cannot fit in {entry.relative_path} ({entry.size_bytes} bytes)"
        )
    changed_bytes = rng.randint(minimum_changed, maximum_changed)
    lengths = _random_composition(changed_bytes, patch_count, 1, rng)
    gaps = _random_composition(
        entry.size_bytes - changed_bytes, patch_count + 1, 0, rng
    )
    patches: list[_BytePatch] = []
    offset = gaps[0]
    for index, length in enumerate(lengths):
        replacement_seed = rng.getrandbits(63)
        replacement = _deterministic_bytes(
            length, replacement_seed, text=not entry.is_binary
        )
        patches.append(_BytePatch(entry.relative_path, offset, replacement))
        offset += length + gaps[index + 1]
    return patches


def _derive_next_version(
    previous_entries: list[_FileEntry],
    ranges: GenerationRanges,
    rng: random.Random,
    long_path_range: IntRange | None,
) -> _VersionChanges:
    removed_paths, added_count, target_total_size = _choose_add_remove_plan(
        previous_entries, ranges, rng, long_path_range
    )
    retained_entries = [
        entry for entry in previous_entries if entry.relative_path not in removed_paths
    ]
    minimum_patchable_size = max(
        ranges.patches_per_file.minimum,
        ranges.changed_bytes_per_file.minimum,
    )
    patchable_entries = [
        entry
        for entry in retained_entries
        if entry.size_bytes >= minimum_patchable_size
    ]
    operation_counts = [
        (changed_count, renamed_count, moved_count)
        for changed_count in range(
            ranges.changed_files.minimum, ranges.changed_files.maximum + 1
        )
        for renamed_count in range(
            ranges.renamed_files.minimum, ranges.renamed_files.maximum + 1
        )
        for moved_count in range(
            ranges.moved_files.minimum, ranges.moved_files.maximum + 1
        )
        if changed_count <= len(patchable_entries)
        and changed_count + renamed_count + moved_count <= len(retained_entries)
    ]
    if not operation_counts:
        raise ValueError(
            "operation ranges require more retained files than are available"
        )
    changed_count, renamed_count, moved_count = rng.choice(operation_counts)
    changed_entries = rng.sample(patchable_entries, changed_count)
    changed_paths = {entry.relative_path for entry in changed_entries}
    operation_pool = [
        entry for entry in retained_entries if entry.relative_path not in changed_paths
    ]
    rng.shuffle(operation_pool)
    renamed_entries = operation_pool[:renamed_count]
    operation_pool = operation_pool[renamed_count:]
    moved_entries = operation_pool[:moved_count]

    paths = {entry.relative_path for entry in retained_entries}
    relocated_paths: dict[str, str] = {}
    for entry in renamed_entries:
        old_path = entry.relative_path
        old = Path(old_path)
        paths.remove(old_path)
        new_name = (
            "file-"
            + _random_name(
                rng,
                12,
                unicode_name=_has_unicode(old_path),
                mixed_case=_has_mixed_case(old_path),
            )
            + old.suffix
        )
        new_path = (old.parent / new_name).as_posix()
        path_keys = {_path_key(path) for path in paths}
        while _path_key(new_path) in path_keys:
            new_name = (
                "file-"
                + _random_name(
                    rng,
                    12,
                    unicode_name=_has_unicode(old_path),
                    mixed_case=_has_mixed_case(old_path),
                )
                + old.suffix
            )
            new_path = (old.parent / new_name).as_posix()
        paths.add(new_path)
        relocated_paths[old_path] = new_path

    for entry in moved_entries:
        old_path = entry.relative_path
        paths.remove(old_path)
        preserve_long_path = (
            long_path_range is not None and len(old_path) >= long_path_range.minimum
        )
        move_depth_range = ranges.path_depth
        if _has_unicode(old_path) or _has_mixed_case(old_path):
            move_depth_range = IntRange(
                max(1, ranges.path_depth.minimum),
                max(1, ranges.path_depth.maximum),
            )
        new_path = _random_relative_path(
            rng,
            move_depth_range,
            paths,
            extension=Path(old_path).suffix,
            filename=Path(old_path).name,
            length_range=IntRange(len(old_path), len(old_path))
            if preserve_long_path
            else None,
            unicode_name=_has_unicode(old_path),
            mixed_case=_has_mixed_case(old_path),
        )
        paths.add(new_path)
        relocated_paths[old_path] = new_path

    patches = [
        patch
        for entry in changed_entries
        for patch in _patches_for_entry(entry, ranges, rng)
    ]

    retained_size = sum(entry.size_bytes for entry in retained_entries)
    added_sizes = (
        _allocate_sizes(
            target_total_size - retained_size,
            added_count,
            ranges.file_size_bytes,
            rng,
        )
        if added_count
        else []
    )
    added_entries: list[_FileEntry] = []
    final_file_count = len(retained_entries) + added_count
    added_indexes = list(range(added_count))

    def added_feature_indexes(
        feature_range: IntRange,
        current_count: int,
        label: str,
    ) -> set[int]:
        target_count = feature_range.capped_choose(rng, final_file_count, label)
        needed = min(added_count, max(0, target_count - current_count))
        return set(rng.sample(added_indexes, needed))

    long_path_indexes = (
        added_feature_indexes(
            ranges.long_path_files,
            sum(len(path) >= long_path_range.minimum for path in paths),
            "long-path files",
        )
        if long_path_range is not None
        else set()
    )
    unicode_indexes = added_feature_indexes(
        ranges.unicode_files,
        sum(_has_unicode(path) for path in paths),
        "Unicode files",
    )
    mixed_case_indexes = added_feature_indexes(
        ranges.mixed_case_files,
        sum(_has_mixed_case(path) for path in paths),
        "mixed-case files",
    )
    for index, size_bytes in enumerate(added_sizes):
        relative_path = _random_relative_path(
            rng,
            ranges.path_depth,
            paths,
            length_range=long_path_range if index in long_path_indexes else None,
            unicode_name=index in unicode_indexes,
            mixed_case=index in mixed_case_indexes,
        )
        paths.add(relative_path)
        added_entries.append(
            _FileEntry(
                relative_path=relative_path,
                size_bytes=size_bytes,
                seed=rng.getrandbits(63),
                is_binary=Path(relative_path).suffix.lower() in _BINARY_EXTENSIONS,
            )
        )

    current_entries = [
        _FileEntry(
            relative_path=relocated_paths.get(entry.relative_path, entry.relative_path),
            size_bytes=entry.size_bytes,
            seed=entry.seed,
            is_binary=entry.is_binary,
        )
        for entry in retained_entries
    ] + added_entries
    return _VersionChanges(
        entries=current_entries,
        removed_paths=removed_paths,
        relocated_paths=relocated_paths,
        patches=patches,
        added_entries=added_entries,
    )


def _replacement_bytes(existing: bytes, proposed: bytes, *, text: bool) -> bytes:
    output = bytearray(proposed)
    alphabet = _TEXT_BYTES if text else bytes(range(256))
    for index, existing_byte in enumerate(existing):
        if output[index] == existing_byte:
            replacement_index = (
                (alphabet.index(existing_byte) + 1) % len(alphabet)
                if existing_byte in alphabet
                else 0
            )
            output[index] = alphabet[replacement_index]
    return bytes(output)


def _build_next_version(
    previous_base_dir: Path,
    next_base_dir: Path,
    previous_entries: list[_FileEntry],
    changes: _VersionChanges,
    *,
    launch_modified_at_ns: int,
) -> None:
    _remove_tree(next_base_dir)
    shutil.copytree(previous_base_dir, next_base_dir)
    _make_tree_writable(next_base_dir)
    for removed_path in changes.removed_paths:
        (next_base_dir / removed_path).unlink()
    entries_by_original_path = {
        entry.relative_path: entry for entry in previous_entries
    }
    for old_path, new_path in changes.relocated_paths.items():
        destination = next_base_dir / new_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        (next_base_dir / old_path).replace(destination)
    for patch in changes.patches:
        entry = entries_by_original_path[patch.relative_path]
        target_path = next_base_dir / patch.relative_path
        with target_path.open("r+b") as handle:
            handle.seek(patch.offset)
            existing = handle.read(len(patch.replacement))
            handle.seek(patch.offset)
            handle.write(
                _replacement_bytes(
                    existing, patch.replacement, text=not entry.is_binary
                )
            )
    for entry in changes.added_entries:
        _write_entry(next_base_dir / entry.relative_path, entry)
    _install_launch_test_executable(next_base_dir, modified_at_ns=launch_modified_at_ns)
    for directory in sorted(
        (path for path in next_base_dir.rglob("*") if path.is_dir()), reverse=True
    ):
        try:
            directory.rmdir()
        except OSError:
            pass


def write_fixture(
    *,
    root: Path,
    name: str,
    total_versions: int,
    append_versions: int,
    force: bool,
    ranges: GenerationRanges | None = None,
    seed: int | None = None,
    emit_test_manifest: bool = False,
    public_source_only: bool = False,
) -> FixtureResult:
    fixture_dir = root / name
    if force and fixture_dir.exists():
        _remove_tree(fixture_dir)
    fixture_dir.mkdir(parents=True, exist_ok=True)

    metadata = _load_fixture_metadata(fixture_dir) or {}
    existing_versions = int(
        metadata.get("latest_version", 0)
    ) or _detect_existing_version_count(fixture_dir)
    if existing_versions:
        stored_ranges = metadata.get("ranges")
        if isinstance(stored_ranges, dict):
            persisted_ranges = GenerationRanges.from_json(stored_ranges)
            if ranges is not None and ranges != persisted_ranges:
                raise ValueError(
                    "cannot change generation ranges while appending versions"
                )
            ranges = persisted_ranges
        if "seed" in metadata:
            persisted_seed = int(metadata["seed"])
            if seed is not None and seed != persisted_seed:
                raise ValueError("cannot change the seed while appending versions")
            seed = persisted_seed
    if ranges is None:
        ranges = GenerationRanges.defaults_for_size_mb(int(metadata.get("size_mb", 50)))
    ranges.validate()
    seed = seed if seed is not None else DEFAULT_SEED

    target_total_versions = (
        existing_versions + append_versions
        if append_versions > 0
        else max(total_versions, existing_versions)
    )
    if target_total_versions <= existing_versions:
        if public_source_only:
            timestamp_base_seconds = _timestamp_base_seconds(seed)
            _write_source_config(
                fixture_dir,
                modified_at_ns=timestamp_base_seconds * 1_000_000_000,
                public_source_only=True,
            )
        if emit_test_manifest:
            timestamp_base_seconds = _timestamp_base_seconds(seed)
            _write_test_manifest(
                fixture_dir,
                seed=seed,
                ranges=ranges,
                timestamp_base_seconds=timestamp_base_seconds,
            )
            _normalize_directory_timestamps(
                fixture_dir,
                modified_at_ns=timestamp_base_seconds * 1_000_000_000,
            )
        return FixtureResult(fixture_dir, [], [], existing_versions, seed)

    long_path_range = _safe_long_path_range(fixture_dir, ranges, target_total_versions)
    timestamp_base_seconds = _timestamp_base_seconds(seed)
    captured_snapshot_dirs: list[Path] = []
    created_version_labels: list[str] = []
    if existing_versions:
        previous_entries = _load_entries_manifest(
            fixture_dir, _version_label(existing_versions)
        )
    else:
        previous_entries = []

    for version_index in range(existing_versions + 1, target_total_versions + 1):
        version_label = _version_label(version_index)
        base_dir = _base_version_dir(fixture_dir, version_label)
        version_rng = random.Random(stable_hash(f"{seed}:version:{version_index}"))
        if version_index == 1:
            current_entries = _initial_entries(ranges, version_rng, long_path_range)
            current_entries = _assign_entry_metadata(
                current_entries, ranges, version_rng, timestamp_base_seconds
            )
            _build_initial_version(
                base_dir,
                current_entries,
                launch_modified_at_ns=_deterministic_timestamp_ns(
                    timestamp_base_seconds,
                    f"{version_label}:LaunchTest",
                ),
            )
        else:
            previous_base_dir = _base_version_dir(
                fixture_dir, _version_label(version_index - 1)
            )
            changes = _derive_next_version(
                previous_entries, ranges, version_rng, long_path_range
            )
            current_entries = _assign_entry_metadata(
                changes.entries, ranges, version_rng, timestamp_base_seconds
            )
            changes.entries = current_entries
            _build_next_version(
                previous_base_dir,
                base_dir,
                previous_entries,
                changes,
                launch_modified_at_ns=_deterministic_timestamp_ns(
                    timestamp_base_seconds,
                    f"{version_label}:LaunchTest",
                ),
            )
        _apply_entry_metadata(base_dir, current_entries)
        _save_entries_manifest(
            fixture_dir,
            version_label,
            current_entries,
            modified_at_ns=_deterministic_timestamp_ns(
                timestamp_base_seconds,
                f"{version_label}:entries",
            ),
        )
        for platform, architecture, tier in SNAPSHOT_VARIANTS:
            captured_snapshot_dirs.append(
                _materialize_tier_snapshot(
                    base_dir,
                    fixture_dir,
                    version_label,
                    platform,
                    architecture,
                    tier,
                    timestamp_base_seconds,
                )
            )
        created_version_labels.append(version_label)
        previous_entries = current_entries

    _save_fixture_metadata(
        fixture_dir,
        {
            "name": name,
            "seed": seed,
            "ranges": ranges.to_json(),
            "latest_version": target_total_versions,
            "snapshot_variants": [
                {"platform": platform, "architecture": architecture, "tier": tier}
                for platform, architecture, tier in SNAPSHOT_VARIANTS
            ],
        },
        modified_at_ns=_deterministic_timestamp_ns(
            timestamp_base_seconds,
            "fixture-metadata",
        ),
    )
    _write_source_config(
        fixture_dir,
        modified_at_ns=_deterministic_timestamp_ns(
            timestamp_base_seconds,
            "source-config",
        ),
        public_source_only=public_source_only,
    )
    if emit_test_manifest:
        _write_test_manifest(
            fixture_dir,
            seed=seed,
            ranges=ranges,
            timestamp_base_seconds=timestamp_base_seconds,
        )
    _normalize_directory_timestamps(
        fixture_dir,
        modified_at_ns=timestamp_base_seconds * 1_000_000_000,
    )
    return FixtureResult(
        fixture_dir,
        captured_snapshot_dirs,
        created_version_labels,
        target_total_versions,
        seed,
    )


def _write_source_config(
    fixture_dir: Path,
    *,
    modified_at_ns: int | None = None,
    public_source_only: bool = False,
) -> None:
    """Write the .gutch_source_config.json ingestion config for the fixture root.

    Default fixtures use auto ingestion mode and ignore the internal .mock-base
    content root. Benchmark fixtures may opt into explicit public-only source
    ingestion so a source root exposes one snapshot per version.
    """
    if public_source_only:
        snapshots = []
        for snapshot_dir in sorted(
            path
            for path in fixture_dir.iterdir()
            if path.is_dir() and path.name.endswith("-public")
        ):
            parsed_snapshot = _parse_snapshot_dir_name(snapshot_dir)
            if parsed_snapshot is None:
                continue
            version_label, platform, architecture, _ = parsed_snapshot
            snapshots.append(
                {
                    "path": snapshot_dir.name,
                    "tags": [
                        f"version={version_label}",
                        f"os={platform}",
                        "arch=x86_64"
                        if architecture == "x86"
                        else f"arch={architecture}",
                    ],
                }
            )
        config = {
            "ingestion_mode": "explicit",
            "auto_ingestion_rules": {"ignore_paths": []},
            "explicit_ingestion_rules": {"snapshots": snapshots},
        }
    else:
        config = {
            "ingestion_mode": "auto",
            "auto_rules": {"ignore_paths": [BASE_CONTENT_ROOT_NAME]},
            "explicit_rules": {"snapshots": []},
        }
    _write_json(
        fixture_dir / ".gutch_source_config.json",
        config,
        modified_at_ns=modified_at_ns,
    )


def _add_range_arguments(
    parser: argparse.ArgumentParser,
    name: str,
    *,
    argument_name: str | None = None,
) -> None:
    argument_name = argument_name or name.replace("_", "-")
    parser.add_argument(f"--{argument_name}-min", dest=f"{name}_min", type=int)
    parser.add_argument(f"--{argument_name}-max", dest=f"{name}_max", type=int)


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate deterministic datasets; use list, prepare CASE or freeze CASE for the corpus"
    )
    parser.add_argument("--repo", "--fixtures-root", dest="path", default="")
    parser.add_argument("--name")
    parser.add_argument(
        "--size-mib",
        "--size-mb",
        dest="size_mb",
        type=int,
        default=0,
        help="Exact product size in MiB shorthand",
    )
    _add_range_arguments(parser, "product_size_mb", argument_name="product-size-mb")
    _add_range_arguments(parser, "file_count")
    _add_range_arguments(parser, "path_depth")
    _add_range_arguments(parser, "file_size_bytes", argument_name="file-size-bytes")
    _add_range_arguments(parser, "changed_files")
    _add_range_arguments(parser, "patches_per_file")
    _add_range_arguments(
        parser, "changed_bytes_per_file", argument_name="changed-bytes"
    )
    _add_range_arguments(parser, "added_files")
    _add_range_arguments(parser, "removed_files")
    _add_range_arguments(parser, "renamed_files")
    _add_range_arguments(parser, "moved_files")
    _add_range_arguments(parser, "long_path_files")
    _add_range_arguments(parser, "relative_path_length")
    _add_range_arguments(parser, "timestamp_age_seconds")
    _add_range_arguments(parser, "readonly_files")
    _add_range_arguments(parser, "executable_files")
    _add_range_arguments(parser, "unicode_files")
    _add_range_arguments(parser, "mixed_case_files")
    parser.add_argument(
        "--seed",
        type=int,
        help=f"Deterministic generation seed (default: {DEFAULT_SEED})",
    )
    parser.add_argument(
        "--emit-test-manifest",
        action="store_true",
        help="Write snapshot paths, sizes, and BLAKE3 hashes for Gutch tests",
    )
    parser.add_argument(
        "--public-source-only",
        action="store_true",
        help="Configure source ingestion to expose only public snapshot variants",
    )
    parser.add_argument("--versions", type=int, default=0)
    parser.add_argument("--append-versions", type=int, default=0)
    parser.add_argument("--force", action="store_true")
    return parser.parse_args()


def _resolve_range(args: argparse.Namespace, name: str, default: IntRange) -> IntRange:
    minimum = getattr(args, f"{name}_min")
    maximum = getattr(args, f"{name}_max")
    if minimum is None and maximum is None:
        return default
    resolved_minimum = minimum if minimum is not None else maximum
    resolved_maximum = maximum if maximum is not None else minimum
    return IntRange(int(resolved_minimum), int(resolved_maximum))


def _ranges_from_args(
    args: argparse.Namespace, default_size_mb: int
) -> GenerationRanges:
    if args.size_mb > 0:
        product_size_mb = IntRange(args.size_mb, args.size_mb)
    else:
        product_size_mb = _resolve_range(
            args,
            "product_size_mb",
            IntRange(default_size_mb, default_size_mb),
        )
    defaults = GenerationRanges.defaults_for_size_mb(product_size_mb.maximum)
    return GenerationRanges(
        product_size_bytes=IntRange(
            product_size_mb.minimum * 1024 * 1024,
            product_size_mb.maximum * 1024 * 1024,
        ),
        file_count=_resolve_range(args, "file_count", defaults.file_count),
        path_depth=_resolve_range(args, "path_depth", defaults.path_depth),
        file_size_bytes=_resolve_range(
            args, "file_size_bytes", defaults.file_size_bytes
        ),
        changed_files=_resolve_range(args, "changed_files", defaults.changed_files),
        patches_per_file=_resolve_range(
            args, "patches_per_file", defaults.patches_per_file
        ),
        changed_bytes_per_file=_resolve_range(
            args, "changed_bytes_per_file", defaults.changed_bytes_per_file
        ),
        added_files=_resolve_range(args, "added_files", defaults.added_files),
        removed_files=_resolve_range(args, "removed_files", defaults.removed_files),
        renamed_files=_resolve_range(args, "renamed_files", defaults.renamed_files),
        moved_files=_resolve_range(args, "moved_files", defaults.moved_files),
        long_path_files=_resolve_range(
            args, "long_path_files", defaults.long_path_files
        ),
        relative_path_length=_resolve_range(
            args, "relative_path_length", defaults.relative_path_length
        ),
        timestamp_age_seconds=_resolve_range(
            args, "timestamp_age_seconds", defaults.timestamp_age_seconds
        ),
        readonly_files=_resolve_range(args, "readonly_files", defaults.readonly_files),
        executable_files=_resolve_range(
            args, "executable_files", defaults.executable_files
        ),
        unicode_files=_resolve_range(args, "unicode_files", defaults.unicode_files),
        mixed_case_files=_resolve_range(
            args, "mixed_case_files", defaults.mixed_case_files
        ),
    )


def main() -> int:
    try:
        if len(sys.argv) > 1 and sys.argv[1] in {"list", "prepare"}:
            parser = argparse.ArgumentParser(
                description="List cases or prepare disposable source data"
            )
            parser.add_argument("action", choices=("list", "prepare"))
            parser.add_argument("case", nargs="?", choices=case_names())
            args = parser.parse_args()
            if args.action == "list":
                for name in case_names():
                    definition = load_case(name)
                    print(
                        f"{name}: {definition['source']['kind']}; repositories: {', '.join(definition['repositories'])}"
                    )
            else:
                if not args.case:
                    parser.error("prepare requires a case name")
                print(prepare_case_source(args.case))
            return 0
        args = _parse_args()
        fixtures_root = Path(args.path or DEFAULT_TEST_DATASET_ROOT).resolve()
        if args.append_versions < 0:
            raise ValueError("append-versions must be >= 0")
        existing = (
            _load_fixture_metadata(fixtures_root / args.name) if args.name else None
        )
        ranges = (
            None
            if existing and not args.force
            else _ranges_from_args(args, max(1, args.size_mb or 4))
        )
        mock_results = [
            write_fixture(
                root=fixtures_root,
                name=args.name or "generated",
                total_versions=args.versions or 2,
                append_versions=args.append_versions,
                force=args.force,
                ranges=ranges,
                seed=args.seed,
                emit_test_manifest=args.emit_test_manifest,
                public_source_only=args.public_source_only,
            )
        ]
        for result in mock_results:
            print(f"=== {result.fixture_dir.name} ===")
            print(f"    Path: {result.fixture_dir}")
            print(f"    Seed: {result.seed}")
            print(f"    Versions: {result.total_versions}")
            print(f"    Generated snapshot dirs: {len(result.captured_snapshot_dirs)}")

    except (OSError, RuntimeError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


@dataclass
class Dataset:
    root: Path
    expected: dict = field(default_factory=dict)

    def directory(self, relative: str, *, read_only=False) -> None:
        path = self.root / relative
        path.mkdir(parents=True, exist_ok=True)
        for parent in [path, *path.parents]:
            if parent == self.root:
                break
            self.expected.setdefault(
                parent.relative_to(self.root).as_posix(),
                {"kind": "directory", "read_only": False},
            )
        if read_only:
            path.chmod(0o555)
            self.expected[relative]["read_only"] = True

    def file(
        self, relative: str, data: bytes, *, read_only=False, executable=False
    ) -> None:
        path = self.root / relative
        if path.parent != self.root:
            self.directory(path.parent.relative_to(self.root).as_posix())
        else:
            self.root.mkdir(parents=True, exist_ok=True)
        if path.exists():
            path.chmod(0o600)
        path.write_bytes(data)
        path.chmod(
            (0o555 if executable else 0o444)
            if read_only
            else (0o755 if executable else 0o644)
        )
        self.expected[relative] = {
            "kind": "file",
            "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
            "read_only": read_only,
            "executable": executable and os.name != "nt",
        }

    def link(self, relative: str, target: str) -> None:
        try:
            (self.root / relative).symlink_to(target)
        except OSError as error:
            raise unittest.SkipTest(f"symbolic links unavailable: {error}") from error
        self.expected[relative] = {"kind": "link", "target": target}

    def save_expectation(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            json.dumps({"schema_version": 1, "entries": self.expected}, indent=2),
            encoding="utf-8",
        )


def basic(root: Path, version: int = 1) -> Dataset:
    data = Dataset(root)
    data.directory("empty directory")
    data.file("empty.txt", b"")
    data.file("data/shared.bin", random.Random(812).randbytes(256 * 1024))
    data.file("data/duplicate.bin", random.Random(812).randbytes(256 * 1024))
    data.file("Unicode cafÃ©/read me.txt", f"version {version}\n".encode())
    data.file("old.txt" if version == 1 else "new.txt", b"renamed content")
    if version >= 3:
        data.file("added.bin", b"third version")
    return data


def metadata(root: Path, *, links=False) -> Dataset:
    data = basic(root)
    data.file("readonly.txt", b"preserve attributes", read_only=True)
    data.file("launch.sh", b"#!/bin/sh\nexit 0\n", executable=True)
    data.directory("readonly-directory", read_only=True)
    data.file(
        "/".join(["long-directory-name-" * 2] * 3) + "/MixedCase.txt",
        b"long nested path",
    )
    if links:
        data.link("shared-link", "data/shared.bin")
    return data


def boundaries(root: Path) -> Dataset:
    data = Dataset(root)
    generator = random.Random(923)
    for size in (
        0,
        1,
        32767,
        32768,
        32769,
        131071,
        131072,
        131073,
        524287,
        524288,
        524289,
    ):
        data.file(f"size-{size}.bin", generator.randbytes(size))
    data.file("repeated.bin", b"repeat" * 100_000)
    return data


def delivery(root: Path) -> Dataset:
    data = Dataset(root)
    generator = random.Random(145)
    # Incompressible content plus the smallest bundle preset ensures multiple artifacts.
    for index in range(8):
        data.file(f"part-{index}.bin", generator.randbytes(1024 * 1024))
    return data


def generated(root: Path, seed: int, version: int = 1) -> Dataset:
    """The default small lifecycle input uses the same fuzz-driven generator."""
    return fuzzed(
        root,
        str(seed).encode(),
        profile="version_changes",
        version=version,
        max_bytes=262144,
    )


GENERATOR_VERSION = 2

# These are input alphabets, not normalization rules. Keep generated code points intact.
NAME_ALPHABETS = (
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
    "éàöñøßÆČŁ",
    "абвгдежзийклмнопрстуфхцчшщэюя",
    "ابتثجحخدذرزسشصضطظعغفقكلمنهوي",
    "אבגדהוזחטיכלמנסעפצקרשת",
    "日本東京漢字語数据文件",
    "αβγδεζηθικλμνξοπρστυφχψω",
    "हिन्दीभाषाफाइल",
    "한국어파일이름",
    "\U0001f680\U0001f600\U0001f9ea\U0001f30d",
    " _-.,;!@#$%&()+={}[]'",
    "e\u0301a\u0308",
    "\u200b\u200d\u2067\u2069",
)


def random_filename(rng, index):
    alphabet = rng.choice(NAME_ALPHABETS)
    # Occasionally draw from all Unicode scalars, excluding filesystem-invalid and
    # control/surrogate values. Explicit ranges keep seeds stable across Unicode versions.
    if rng.randrange(4) == 0:
        points = []
        while len(points) < 8:
            character = chr(rng.randrange(0x110000))
            if (
                ord(character) >= 160
                and not 0xD800 <= ord(character) <= 0xDFFF
                and character not in '/\\<>:"|?*'
            ):
                points.append(character)
        alphabet += "".join(points)
    body = "".join(rng.choice(alphabet) for _ in range(rng.randint(1, 24)))
    return f"n{index}-{body}.dat"


FUZZ_PROFILES = (
    "empty_and_tiny",
    "chunk_boundaries",
    "duplicates",
    "compressible",
    "random_content",
    "unicode",
    "nested_paths",
    "attributes",
    "version_changes",
    "many_small_files",
)


def fuzzed(
    root: Path,
    input_bytes: bytes,
    *,
    profile="random_content",
    version=1,
    max_bytes=1024 * 1024,
) -> Dataset:
    """Turn arbitrary fuzzer bytes into a bounded, reproducible source tree."""
    if (
        profile not in FUZZ_PROFILES
        or not 128 <= max_bytes <= 4 * 1024 * 1024
        or not 1 <= version <= 100
    ):
        raise ValueError(
            "invalid fuzz profile, byte budget (128..4 MiB), or version (1..100)"
        )
    if root.exists() and any(root.iterdir()):
        raise ValueError("fuzz generation requires an empty destination")
    rng = random.Random(int.from_bytes(hashlib.sha256(input_bytes).digest(), "big"))
    data = Dataset(root)
    remaining = max_bytes

    def emit(name, content, **attributes):
        nonlocal remaining
        content = content[:remaining]
        data.file(name, content, **attributes)
        remaining -= len(content)

    data.directory("empty")
    emit("version.txt", f"{version}\n".encode())
    emit("empty.bin", b"")
    shared = rng.randbytes(min(65537, remaining // 4))
    emit("shared.bin", shared)
    reserved = remaining // 3
    remaining -= reserved
    if profile == "empty_and_tiny":
        for index in range(20):
            emit(f"tiny-{index}.bin", rng.randbytes(rng.randrange(17)))
    elif profile == "chunk_boundaries":
        for size in (1, 32767, 32768, 32769, 131071, 131072, 131073):
            emit(f"boundary-{size}.bin", rng.randbytes(size))
    elif profile == "duplicates":
        for index in range(6):
            emit(f"duplicate-{index}.bin", shared)
    elif profile == "compressible":
        emit("zeros.bin", bytes(remaining // 2))
        emit("repeated.bin", b"repeat" * (remaining // 6))
    elif profile == "unicode":
        for name in ("café", "東京", "данные", "space and MixedCase"):
            emit(f"{name}/content.txt", rng.randbytes(777))
    elif profile == "nested_paths":
        for depth in range(1, 6):
            emit("/".join(["nested"] * depth) + "/MixedCase.bin", rng.randbytes(1025))
    elif profile == "attributes":
        emit("readonly.txt", b"preserve permissions", read_only=True)
        emit("launch.sh", b"#!/bin/sh\nexit 0\n", executable=True)
        data.directory("readonly-directory", read_only=True)
    elif profile == "many_small_files":
        for index in range(100):
            emit(
                f"group-{index % 5}/file-{index}.bin",
                rng.randbytes(rng.randrange(2049)),
            )
    else:
        emit("random.bin", rng.randbytes(remaining // 2))
    remaining += reserved
    name_rng = random.Random(
        int.from_bytes(hashlib.sha256(input_bytes + b"names-v2").digest(), "big")
    )
    # Guaranteed representative scripts plus random combinations in every case.
    for index, alphabet in enumerate(NAME_ALPHABETS):
        emit(
            f"names/n{index}-{alphabet}.txt"
            if index not in (10,)
            else "names/n10-punctuation !@#$%&()+={}[]'.txt",
            name_rng.randbytes(name_rng.randrange(65)),
            read_only=index % 4 == 0,
            executable=index % 4 == 1,
        )
    for index in range(name_rng.randint(12, 28)):
        depth = name_rng.randint(0, 5)
        parents = [
            f"d{level}-" + random_filename(name_rng, index) for level in range(depth)
        ]
        name = "/".join(["random", *parents, random_filename(name_rng, index)])
        emit(
            name,
            name_rng.randbytes(name_rng.choice((0, 1, 31, 127, 1023, 4097))),
            read_only=name_rng.randrange(4) == 0,
            executable=name_rng.randrange(4) == 0,
        )
    # Metadata-only changes, type replacements and case changes are explicit transitions.
    emit(
        "permission-change.txt",
        b"same bytes across versions",
        read_only=version % 2 == 0,
    )
    if version % 2:
        emit("type-change", b"file becomes directory")
    else:
        emit("type-change/child", b"directory replaces file")
    emit("Case-change.txt" if version % 2 else "case-change.txt", b"case-only rename")

    # Different versions exercise insertion, deletion, renaming, moves and stable tails.
    rng.seed(
        int.from_bytes(
            hashlib.sha256(input_bytes + str(version).encode()).digest(), "big"
        )
    )
    offset = rng.randrange(len(shared)) if shared else 0
    emit(
        "changing.bin",
        shared[:offset] + rng.randbytes(version * 13) + shared[offset + version :],
    )
    emit("old.txt" if version == 1 else "moved/new.txt", b"identical renamed content")
    emit("removed.txt" if version == 1 else "added.txt", rng.randbytes(17))
    return data


def case_names():
    return sorted(path.parent.name for path in CORPUS_ROOT.glob("*/case.json"))


def load_case(name):
    if name not in case_names():
        raise ValueError(f"unknown dataset case: {name}")
    definition = json.loads(
        (CORPUS_ROOT / name / "case.json").read_text(encoding="utf-8")
    )
    if definition.get("schema_version") != 1:
        raise ValueError(f"unsupported dataset schema: {name}")
    return definition


def prepare_case_source(name):
    """Prepare only disposable sources, never overwrite committed corpus files."""
    definition = load_case(name)
    settings = definition["source"]
    target = TESTS_ROOT.parent / ".artifacts/tests/datasets" / name / "source"
    if settings["kind"] == "blender":
        from .prepare_blender import prepare_blender_product

        prepare_blender_product(
            recipe_path=CORPUS_ROOT / name / "case.json",
            product_dir=target,
            versions=settings["selected_versions"],
            platforms=settings["selected_platforms"],
            cache_dir=target.parent / "archives",
        )
    elif settings["kind"] == "generated":
        if settings["generator_version"] != GENERATOR_VERSION:
            raise ValueError("corpus requires a different generator version")
        if not target.exists():
            for version in range(1, settings["versions"] + 1):
                fuzzed(
                    target / f"v{version}",
                    bytes.fromhex(settings["input_hex"]),
                    profile=settings["profile"],
                    version=version,
                    max_bytes=settings["max_bytes_per_version"],
                )
    else:
        raise ValueError(
            f"case {name} preserves a historical repository without regenerable source"
        )
    if (CORPUS_ROOT / name / "expected.json").exists():
        validate_case_source(name, target)
    return target


def validate_case_source(name, target):
    """Check prepared inputs against frozen source inventories, independent of Gutch."""
    expected = json.loads(
        (CORPUS_ROOT / name / "expected.json").read_text(encoding="utf-8")
    )
    for version, specification in expected.items():
        root = target / version
        for relative, entry in specification["entries"].items():
            path = root / relative
            if entry["kind"] == "file":
                if not path.is_file() or path.is_symlink():
                    raise ValueError(f"missing source file: {path}")
                with path.open("rb") as stream:
                    digest = hashlib.file_digest(stream, "sha256").hexdigest()
                if digest != entry["sha256"]:
                    raise ValueError(f"source differs from frozen inventory: {path}")
            elif entry["kind"] == "directory" and not path.is_dir():
                raise ValueError(f"missing source directory: {path}")

        def fail(error):
            raise error

        actual = {
            str((Path(directory) / name).relative_to(root)).replace("\\", "/")
            for directory, directories, files in os.walk(root, onerror=fail)
            for name in directories + files
        }
        if actual != specification["entries"].keys():
            raise ValueError(f"source paths differ from frozen inventory: {root}")
    return target


def prepared_source(name):
    load_case(name)
    target = TESTS_ROOT.parent / ".artifacts/tests/datasets" / name / "source"
    if not target.is_dir():
        raise ValueError(
            f"missing prepared source; run: python gutch.py dataset prepare {name}"
        )
    return validate_case_source(name, target)


if __name__ == "__main__":
    sys.exit(main())
