"""Gutch CLI command construction."""

from pathlib import Path

from .models import GutchProfile


class GutchCli:
    def __init__(self, executable: Path, *, tracing: bool = True):
        self.executable = executable
        self.tracing = tracing

    def command(self, *arguments: str | Path) -> list[str | Path]:
        command = [self.executable, *arguments]
        if not self.tracing and "--trace-out" in command:
            index = command.index("--trace-out")
            del command[index:index + 2]
        return command

    @staticmethod
    def profile_args(profile: GutchProfile) -> list[str]:
        arguments = [
            "--chunk-profile",
            profile.chunk_profile,
            "--bundle-profile",
            profile.bundle_profile,
            "--compression-algorithm",
            profile.compression_algorithm,
            "--storage-mode",
            profile.storage_mode,
        ]
        if profile.compression_level is not None:
            arguments.extend(["--compression-level", str(profile.compression_level)])
        if profile.jobs is not None:
            arguments.extend(["--jobs", str(profile.jobs)])
        return arguments

    @staticmethod
    def runtime_args(profile: GutchProfile) -> list[str]:
        return ["--jobs", str(profile.jobs)] if profile.jobs is not None else []
