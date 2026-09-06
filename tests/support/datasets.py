"""Small deterministic recipes. Expectations are derived from input bytes, not Gutch."""
from __future__ import annotations

from dataclasses import dataclass, field
import hashlib
import os
from pathlib import Path
import random
import unittest

from .filesystem import write_json


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
            self.expected.setdefault(parent.relative_to(self.root).as_posix(), {"kind": "directory", "read_only": False})
        if read_only:
            path.chmod(0o555)
            self.expected[relative]["read_only"] = True

    def file(self, relative: str, data: bytes, *, read_only=False, executable=False) -> None:
        path = self.root / relative
        if path.parent != self.root:
            self.directory(path.parent.relative_to(self.root).as_posix())
        else:
            self.root.mkdir(parents=True, exist_ok=True)
        if path.exists():
            path.chmod(0o600)
        path.write_bytes(data)
        path.chmod(0o444 if read_only else (0o755 if executable else 0o644))
        self.expected[relative] = {"kind": "file", "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(), "read_only": read_only,
            "executable": executable and os.name != "nt"}

    def link(self, relative: str, target: str) -> None:
        try:
            (self.root / relative).symlink_to(target)
        except OSError as error:
            raise unittest.SkipTest(f"symbolic links unavailable: {error}") from error
        self.expected[relative] = {"kind": "link", "target": target}

    def save_expectation(self, path: Path) -> None:
        write_json(path, {"schema_version": 1, "entries": self.expected})


def basic(root: Path, version: int = 1) -> Dataset:
    data = Dataset(root)
    data.directory("empty directory")
    data.file("empty.txt", b"")
    data.file("data/shared.bin", random.Random(812).randbytes(256 * 1024))
    data.file("data/duplicate.bin", random.Random(812).randbytes(256 * 1024))
    data.file("Unicode café/read me.txt", f"version {version}\n".encode())
    data.file("old.txt" if version == 1 else "new.txt", b"renamed content")
    if version >= 3:
        data.file("added.bin", b"third version")
    return data


def metadata(root: Path, *, links=False) -> Dataset:
    data = basic(root)
    data.file("readonly.txt", b"preserve attributes", read_only=True)
    data.file("launch.sh", b"#!/bin/sh\nexit 0\n", executable=True)
    data.directory("readonly-directory", read_only=True)
    data.file("/".join(["long-directory-name-" * 2] * 3) + "/MixedCase.txt", b"long nested path")
    if links:
        data.link("shared-link", "data/shared.bin")
    return data


def boundaries(root: Path) -> Dataset:
    data = Dataset(root)
    generator = random.Random(923)
    for size in (0, 1, 32767, 32768, 32769, 131071, 131072, 131073, 524287, 524288, 524289):
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
