"""One SSH/SCP boundary for remote benchmark adapters."""

from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from ..common.command import CommandResult, CommandRunner


@dataclass(frozen=True)
class RemoteTarget:
    name: str
    host: str
    ssh_config: Path | None = None
    workspace: str = "/tmp/hostapp-benchmark"


class RemoteLauncher:
    def __init__(self, target: RemoteTarget, commands: CommandRunner | None = None):
        self.target = target
        self.commands = commands or CommandRunner()

    def _ssh_prefix(self) -> list[str]:
        prefix = ["ssh"]
        if self.target.ssh_config:
            prefix.extend(["-F", str(self.target.ssh_config)])
        prefix.append(self.target.host)
        return prefix

    def run(self, command: Sequence[str], *, description: str) -> CommandResult:
        remote = " ".join(self._quote(item) for item in command)
        return self.commands.run(
            [*self._ssh_prefix(), remote],
            description=description,
        )

    def upload(self, source: Path, destination: str, *, description: str) -> CommandResult:
        prefix = ["scp"]
        if self.target.ssh_config:
            prefix.extend(["-F", str(self.target.ssh_config)])
        prefix.extend([str(source), f"{self.target.host}:{destination}"])
        return self.commands.run(prefix, description=description)

    def download(self, source: str, destination: Path, *, description: str) -> CommandResult:
        prefix = ["scp"]
        if self.target.ssh_config:
            prefix.extend(["-F", str(self.target.ssh_config)])
        prefix.extend([f"{self.target.host}:{source}", str(destination)])
        return self.commands.run(prefix, description=description)

    @staticmethod
    def _quote(value: str) -> str:
        return "'" + value.replace("'", "'\\''") + "'"
