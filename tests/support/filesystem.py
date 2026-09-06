"""Independent SHA-256 tree comparison and operation-owned cleanup."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import stat


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def inventory(root: Path) -> dict:
    entries = {}
    for directory, directories, files in os.walk(root, followlinks=False):
        for name in sorted(directories + files):
            path = Path(directory) / name
            relative = path.relative_to(root).as_posix()
            mode = path.lstat().st_mode
            if stat.S_ISLNK(mode):
                target = os.path.normpath(str(Path(relative).parent / os.readlink(path))).replace("\\", "/")
                entries[relative] = {"kind": "link", "target": target}
            elif stat.S_ISDIR(mode):
                entries[relative] = {"kind": "directory", "read_only": not bool(mode & stat.S_IWUSR)}
            elif stat.S_ISREG(mode):
                with path.open("rb") as stream:
                    digest = hashlib.file_digest(stream, "sha256").hexdigest()
                entries[relative] = {"kind": "file", "size": path.stat().st_size,
                    "sha256": digest, "read_only": not bool(mode & stat.S_IWUSR),
                    "executable": bool(mode & 0o111) if os.name != "nt" else False}
            else:
                raise AssertionError(f"unsupported filesystem entry: {path}")
    return entries


def assert_tree(expected: dict, actual: Path, evidence: Path) -> None:
    observed = inventory(actual) if actual.is_dir() else {}
    differences = [f"missing: {p}" for p in sorted(expected.keys() - observed.keys())]
    differences += [f"unexpected: {p}" for p in sorted(observed.keys() - expected.keys())]
    differences += [f"different: {p}" for p in sorted(expected.keys() & observed.keys()) if expected[p] != observed[p]]
    write_json(evidence, {"passed": actual.is_dir() and not differences, "differences": differences})
    assert actual.is_dir() and not differences, f"tree mismatch at {actual}: {differences[:10]}"


def remove_owned(path: Path, owner: Path) -> None:
    # Validate the resolved target before recursive deletion; never follow a root link.
    owner = owner.resolve()
    if path.is_symlink():
        assert path.parent.resolve().is_relative_to(owner)
        path.unlink()
        return
    target = path.resolve()
    if target == owner or not target.is_relative_to(owner):
        raise ValueError(f"cleanup target is not a child of its owner: {path}")
    if not target.exists():
        return
    for directory, directories, files in os.walk(target, followlinks=False):
        for name in directories + files:
            child = Path(directory) / name
            if not child.is_symlink():
                child.chmod(child.stat().st_mode | stat.S_IWUSR | stat.S_IXUSR)
    target.chmod(target.stat().st_mode | stat.S_IWUSR | stat.S_IXUSR)
    shutil.rmtree(target)
