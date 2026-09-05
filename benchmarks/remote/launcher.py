"""One SSH/SCP boundary for remote benchmark adapters."""

from dataclasses import dataclass
from pathlib import Path
from collections.abc import Callable, Sequence

from ..benchmark_config import load_benchmark_config
from ..common.command import CommandResult, CommandRunner


@dataclass(frozen=True)
class RemoteTarget:
    name: str
    host: str
    ssh_config: Path | None = None
    port: int | None = None
    identity_file: Path | None = None
    workspace: str = "/tmp/hostapp-benchmark"

    @classmethod
    def from_config(cls, name: str | None = None, config_path: Path | None = None) -> "RemoteTarget":
        config = load_benchmark_config(config_path)
        target = config.ssh_target(name)
        return cls(
            name=target.name,
            host=target.destination,
            port=target.port,
            identity_file=target.identity_file,
            workspace=config.remote_workspace_root,
        )


class RemoteLauncher:
    def __init__(self, target: RemoteTarget, commands: CommandRunner | None = None):
        self.target = target
        self.commands = commands or CommandRunner()

    def _ssh_prefix(self) -> list[str]:
        prefix = ["ssh"]
        if self.target.ssh_config:
            prefix.extend(["-F", str(self.target.ssh_config)])
        if self.target.port:
            prefix.extend(["-p", str(self.target.port)])
        if self.target.identity_file:
            prefix.extend(["-i", str(self.target.identity_file)])
        prefix.append(self.target.host)
        return prefix

    def run(
        self,
        command: Sequence[str],
        *,
        description: str,
        on_output: Callable[[str, bool], None] | None = None,
    ) -> CommandResult:
        remote = " ".join(self._quote(item) for item in command)
        return self.commands.run(
            [*self._ssh_prefix(), remote],
            description=description,
            on_output=on_output,
        )

    def upload(self, source: Path, destination: str, *, description: str) -> CommandResult:
        prefix = ["scp"]
        if self.target.ssh_config:
            prefix.extend(["-F", str(self.target.ssh_config)])
        if self.target.port:
            prefix.extend(["-P", str(self.target.port)])
        if self.target.identity_file:
            prefix.extend(["-i", str(self.target.identity_file)])
        prefix.extend([str(source), f"{self.target.host}:{destination}"])
        return self.commands.run(prefix, description=description)

    def download(self, source: str, destination: Path, *, description: str) -> CommandResult:
        prefix = ["scp"]
        if self.target.ssh_config:
            prefix.extend(["-F", str(self.target.ssh_config)])
        if self.target.port:
            prefix.extend(["-P", str(self.target.port)])
        if self.target.identity_file:
            prefix.extend(["-i", str(self.target.identity_file)])
        prefix.extend([f"{self.target.host}:{source}", str(destination)])
        return self.commands.run(prefix, description=description)

    @staticmethod
    def _quote(value: str) -> str:
        return "'" + value.replace("'", "'\\''") + "'"
