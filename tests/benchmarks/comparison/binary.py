"""Resolve a built standalone gutch-cli without embedding benchmark-specific paths."""

from pathlib import Path
import os
import subprocess


def _iter_build_inputs(repo_root: Path):
    files = [
        repo_root / "Cargo.toml",
        repo_root / "Cargo.lock",
        repo_root / "gutch-cli" / "Cargo.toml",
        repo_root / "gutch-cli" / "Cargo.lock",
    ]
    directories = [
        repo_root / "src",
        repo_root / "gutch-cli" / "src",
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
            raise FileNotFoundError(
                f"configured Gutch executable does not exist: {explicit}"
            )
        return explicit
    executable_name = "gutch-cli.exe" if os.name == "nt" else "gutch-cli"
    candidates: list[Path | None] = []
    candidates.extend(
        [
            repo_root
            / ".builds"
            / "compiler-cache"
            / "cargo"
            / "release"
            / executable_name,
            repo_root / "target" / "release" / executable_name,
            repo_root / "tests/benchmarks" / "comparison" / "bin" / executable_name,
        ]
    )
    for candidate in candidates:
        if candidate and _binary_is_current(candidate, repo_root):
            return candidate
    cargo_target = repo_root / ".builds" / "compiler-cache" / "cargo"
    build_env = os.environ.copy()
    build_env["CARGO_TARGET_DIR"] = str(cargo_target)
    subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--locked",
            "--manifest-path",
            repo_root / "gutch-cli" / "Cargo.toml",
            "--bin",
            "gutch-cli",
        ],
        cwd=repo_root,
        env=build_env,
        check=True,
    )
    # Cargo's successful freshness check is authoritative: cfg(test)-only edits
    # can be newer than a release binary without requiring a release rebuild.
    built = cargo_target / "release" / executable_name
    if built.is_file():
        return built
    raise FileNotFoundError("could not locate gutch-cli after release build")
