"""Resolve a built hostapp-cli without embedding benchmark-specific paths."""

from pathlib import Path
import os
import subprocess


def _iter_build_inputs(repo_root: Path):
    files = [
        repo_root / "host_app" / "hostapp-cli" / "Cargo.toml",
        repo_root / "host_app" / "hostapp-cli" / "Cargo.lock",
        repo_root / "gutch_repo" / "Cargo.toml",
        repo_root / "host_app" / "hostapp-auth" / "Cargo.toml",
        repo_root / "host_app" / "hostapp-common" / "Cargo.toml",
    ]
    directories = [
        repo_root / "host_app" / "hostapp-cli" / "src",
        repo_root / "gutch_repo" / "src",
        repo_root / "host_app" / "hostapp-auth" / "src",
        repo_root / "host_app" / "hostapp-common" / "src",
    ]
    yield from (path for path in files if path.is_file())
    for directory in directories:
        if directory.is_dir():
            yield from (path for path in directory.rglob("*") if path.is_file())


def _binary_is_current(candidate: Path, repo_root: Path) -> bool:
    if not candidate.is_file():
        return False
    try:
        binary_mtime = candidate.stat().st_mtime
    except OSError:
        return False
    for path in _iter_build_inputs(repo_root):
        try:
            if path.stat().st_mtime > binary_mtime:
                return False
        except OSError:
            return False
    return True


def resolve_gutch_executable(repo_root: Path, explicit: Path | None = None) -> Path:
    if explicit is not None:
        if not explicit.exists():
            raise FileNotFoundError(f"configured Gutch executable does not exist: {explicit}")
        return explicit
    candidates: list[Path | None] = []
    candidates.extend(
        [
            repo_root / ".build" / "main" / "host_app" / "intermediates" / "cargo" / "default" / "release" / "hostapp-cli.exe",
            repo_root / ".build" / "main" / "app" / "cargo" / "default" / "release" / "hostapp-cli.exe",
            repo_root / "target" / "release" / "hostapp-cli.exe",
            repo_root / "gutch_repo" / "tests" / "benchmarks" / "gutch_fixture_runner" / "bin" / "hostapp-cli.exe",
        ]
    )
    for candidate in candidates:
        if candidate and _binary_is_current(candidate, repo_root):
            return candidate
    cargo_target = repo_root / ".build" / "main" / "host_app" / "intermediates" / "cargo" / "default"
    build_env = os.environ.copy()
    build_env["CARGO_TARGET_DIR"] = str(cargo_target)
    subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--manifest-path",
            repo_root / "host_app" / "hostapp-cli" / "Cargo.toml",
            "--bin",
            "hostapp-cli",
        ],
        cwd=repo_root,
        env=build_env,
        check=True,
    )
    for candidate in candidates:
        if candidate and _binary_is_current(candidate, repo_root):
            return candidate
    raise FileNotFoundError("could not locate hostapp-cli after release build")
