"""Real CLI execution, command deadlines and per-case isolated configuration."""
from __future__ import annotations

from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import re
import subprocess
import time

from .process import stop_process
from .filesystem import assert_tree, write_json

ROOT = Path(__file__).resolve().parents[2]
REPOSITORY = ROOT


def start_process(command, directory, environment, *, log_path):
    # Keep POSIX descendants in the scenario worker's process group for hard timeouts.
    # Automated commands must never inherit an interactive confirmation prompt.
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("a", encoding="utf-8") as log:
        return subprocess.Popen(command, cwd=directory, env={**os.environ, **environment},
                                stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0,
                                start_new_session=os.name != "nt" and not os.environ.get("GUTCH_SCENARIO_WORKER"))


def execute(command, directory: Path, environment: dict, log: Path, *, timeout=180, success=True):
    command = [str(part) for part in command]
    started = time.monotonic()
    process = start_process(command, directory, environment, log_path=log)
    try:
        code = process.wait(timeout=timeout)
    except BaseException:
        stop_process(process)
        raise
    finally:
        # Arguments can contain signed URLs. Logs have no full command or credentials.
        write_json(log.with_suffix(".json"), {"exit_code": process.poll(), "seconds": time.monotonic() - started})
    if success is True and code != 0:
        raise AssertionError(f"command failed ({code}); see {log}\n{log.read_text(encoding='utf-8', errors='replace')[-2000:]}")
    if success is False and code == 0:
        raise AssertionError(f"expected command failure; see {log}")
    return code


@dataclass
class Case:
    root: Path
    executable: Path
    sequence: int = 0
    checks: list[str] = field(default_factory=list)
    details: dict = field(default_factory=dict)

    def __post_init__(self):
        self.payload.mkdir(parents=True, exist_ok=True)

    @property
    def payload(self):
        return self.root / "payload"

    @property
    def environment(self):
        return {"XDG_CONFIG_HOME": str(self.payload / "config"),
                "XDG_DATA_HOME": str(self.payload / "data"), "XDG_CACHE_HOME": str(self.payload / "cache")}

    def log(self, label):
        self.sequence += 1
        return self.root / "commands" / f"{self.sequence:03d}-{label}.log"

    def command(self, *arguments, success=True):
        return execute([self.executable, *arguments], REPOSITORY, self.environment,
                       self.log(str(arguments[0])), success=success)

    def stats(self, *arguments, success=True):
        path = self.root / "commands" / f"{self.sequence + 1:03d}-stats.json"
        self.command(*arguments, "--json", "--output", path, success=success)
        return json.loads(path.read_text(encoding="utf-8"))

    def init(self, name="repo"):
        repo = self.payload / name
        self.command("repo", "init", "--repo", repo)
        return repo

    def capture(self, repo, source, *options):
        stats = self.stats("snapshot", "capture", "--repo", repo, "--source", source,
                           "--yes", "--jobs", "2", *options)
        assert re.fullmatch(r"[0-9a-f]{64}", stats["snapshot_id"])
        return stats

    def reconstruct(self, repo, snapshot, destination, *options, success=True):
        return self.stats("snapshot", "reconstruct", "--repo", repo, "--snapshot", snapshot,
                          "--destination", destination, "--jobs", "2", *options, success=success)

    def verify(self, repo, snapshot, destination, *, success=True):
        return self.stats("installation", "verify", "--repo", repo, "--snapshot", snapshot,
                          "--destination", destination, "--strict", success=success)

    def compare(self, dataset, destination, label="output"):
        dataset.save_expectation(self.root / f"{label}-expected.json")
        assert_tree(dataset.expected, destination, self.root / f"{label}-comparison.json")
        self.checks.append(label)

    def repair(self, repo, snapshot, destination, *options):
        return self.stats("installation", "repair", "--repo", repo, "--snapshot", snapshot,
                          "--destination", destination, "--yes", *options)
