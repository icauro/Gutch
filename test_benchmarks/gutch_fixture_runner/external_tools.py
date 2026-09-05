"""Pinned Linux x86_64 binaries for remote external-tool benchmarks."""

from __future__ import annotations

from dataclasses import dataclass
import bz2
import hashlib
from pathlib import Path
import shutil
import stat
import tarfile
import tempfile
from urllib.request import urlopen


@dataclass(frozen=True)
class LinuxToolRelease:
    name: str
    version: str
    url: str
    sha256: str
    archive: str


LINUX_TOOL_RELEASES = {
    "restic": LinuxToolRelease(
        "restic",
        "0.19.1",
        "https://github.com/restic/restic/releases/download/v0.19.1/restic_0.19.1_linux_amd64.bz2",
        "f415415624dcc452f2a02b8c33641791a8c6d6d3b65bbb3543fcf9a25151585c",
        "bz2",
    ),
    "kopia": LinuxToolRelease(
        "kopia",
        "0.23.1",
        "https://github.com/kopia/kopia/releases/download/v0.23.1/kopia-0.23.1-linux-x64.tar.gz",
        "416d0f84a3dbb321a8b2d8f0997b1a0a6e915babe79ee76fa6e4d2bd1e1c5178",
        "tar.gz",
    ),
    "borg": LinuxToolRelease(
        "borg",
        "1.4.5",
        "https://github.com/borgbackup/borg/releases/download/1.4.5/borg-linux-glibc235-x86_64-gh",
        "1410e28609be3080d0e3ab27a78f5c586a29fd0c75c2aa6415fcde1293bcd923",
        "binary",
    ),
    "plakar": LinuxToolRelease(
        "plakar",
        "1.1.5",
        "https://github.com/PlakarKorp/plakar/releases/download/v1.1.5/plakar_1.1.5_linux_amd64.tar.gz",
        "36d9bbd95ce3735a50af916cf5739e97b21a734a79cfaca7a94a774095a8b5a4",
        "tar.gz",
    ),
}

LINUX_TOOL_DIRECTORY = "linux-x64"
LINUX_TOOL_MANIFEST = ".versions.json"


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _download(url: str, destination: Path) -> None:
    with urlopen(url, timeout=120) as response, destination.open("wb") as output:
        shutil.copyfileobj(response, output, length=1024 * 1024)


def _extract_tar_binary(archive_path: Path, destination: Path, name: str) -> Path:
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive_path, mode="r:gz") as archive:
        for member in archive.getmembers():
            member_path = (destination / member.name).resolve()
            if destination.resolve() not in member_path.parents:
                raise RuntimeError(f"unsafe Linux tool archive entry: {member.name}")
            if member.issym() or member.islnk():
                raise RuntimeError(f"refusing linked Linux tool archive entry: {member.name}")
        archive.extractall(destination)
    candidates = [path for path in destination.rglob(name) if path.is_file()]
    if len(candidates) != 1:
        raise RuntimeError(f"could not identify {name} in {archive_path.name}")
    return candidates[0]


def _install_release(release: LinuxToolRelease, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=f"{release.name}-", dir=destination.parent) as temporary:
        temporary_root = Path(temporary)
        archive_path = temporary_root / f"{release.name}.{release.archive}"
        print(f"Downloading Linux {release.name} {release.version}...", flush=True)
        _download(release.url, archive_path)
        if _sha256(archive_path) != release.sha256:
            raise RuntimeError(f"SHA-256 mismatch for Linux {release.name} release")
        if release.archive == "binary":
            shutil.copy2(archive_path, destination)
        elif release.archive == "bz2":
            with bz2.open(archive_path, "rb") as source, destination.open("wb") as output:
                shutil.copyfileobj(source, output, length=1024 * 1024)
        else:
            extracted_binary = _extract_tar_binary(archive_path, temporary_root / "extracted", release.name)
            shutil.copy2(extracted_binary, destination)
        destination.chmod(destination.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def ensure_linux_external_tools(repo_root: Path, kinds: set[str]) -> dict[str, Path]:
    unknown = kinds - LINUX_TOOL_RELEASES.keys()
    if unknown:
        raise ValueError(f"unsupported Linux external benchmark tools: {sorted(unknown)}")
    directory = repo_root / "gutch_repo" / "benchmarks" / "gutch_fixture_runner" / "bin" / LINUX_TOOL_DIRECTORY
    manifest_path = directory / LINUX_TOOL_MANIFEST
    existing = {}
    if manifest_path.is_file():
        import json

        existing = json.loads(manifest_path.read_text(encoding="utf-8"))
    paths = {kind: directory / kind for kind in kinds}
    if not all(path.is_file() for path in paths.values()) or any(
        existing.get(kind) != LINUX_TOOL_RELEASES[kind].version for kind in kinds
    ):
        for kind in sorted(kinds):
            _install_release(LINUX_TOOL_RELEASES[kind], paths[kind])
        import json

        directory.mkdir(parents=True, exist_ok=True)
        manifest_path.write_text(
            json.dumps(
                {kind: LINUX_TOOL_RELEASES[kind].version for kind in sorted(kinds)},
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
    return paths


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Install pinned Linux benchmark tools")
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[3])
    parser.add_argument("tools", nargs="+", choices=sorted(LINUX_TOOL_RELEASES))
    arguments = parser.parse_args()
    ensure_linux_external_tools(arguments.repo_root.resolve(), set(arguments.tools))
