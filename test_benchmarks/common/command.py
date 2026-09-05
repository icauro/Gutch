"""One command execution boundary for all benchmark workflows."""

from dataclasses import dataclass
import itertools
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Mapping, Sequence
from typing import TextIO
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit


@dataclass(frozen=True)
class CommandResult:
    command: tuple[str, ...]
    elapsed_seconds: float
    returncode: int
    stdout: str
    stderr: str


class CommandError(RuntimeError):
    def __init__(self, result: CommandResult, description: str):
        self.result = result
        self.description = description
        rendered = " ".join(result.command)
        super().__init__(
            f"{description} failed with exit code {result.returncode}: {rendered}\n"
            f"{result.stderr.strip()}"
        )


class _CommandProgress:
    """Small terminal spinner that does not pollute redirected benchmark output."""

    def __init__(self, description: str, stream: TextIO, enabled: bool | None):
        self.description = description
        self.stream = stream
        self.enabled = stream.isatty() if enabled is None else enabled
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._finished = False

    def start(self) -> None:
        if not self.enabled:
            return
        self._write("|", end="")
        self._thread = threading.Thread(target=self._spin, daemon=True)
        self._thread.start()

    def _spin(self) -> None:
        for frame in itertools.cycle(("/", "-", "\\", "|")):
            if self._stop.wait(0.12):
                return
            self._write(frame, end="")

    def _write(self, frame: str, *, end: str) -> None:
        print(f"\r[{frame}] {self.description}", end=end, file=self.stream, flush=True)

    def finish(self, success: bool) -> None:
        if self._finished:
            return
        self._finished = True
        if not self.enabled:
            return
        self._stop.set()
        if self._thread is not None:
            self._thread.join()
        self._write("done" if success else "FAIL", end="\n")


def _redact(value: str) -> str:
    try:
        parsed = urlsplit(value)
    except ValueError:
        return value
    if not parsed.scheme or not parsed.netloc or not parsed.query:
        return value
    query = [(key, "<redacted>") for key, _ in parse_qsl(parsed.query, keep_blank_values=True)]
    return urlunsplit((parsed.scheme, parsed.netloc, parsed.path, urlencode(query), parsed.fragment))


class CommandRunner:
    def __init__(
        self,
        *,
        progress: bool | None = None,
        stream: TextIO | None = None,
        step_wait_seconds: float = 0.0,
        on_step: Callable[[str], None] | None = None,
    ):
        self.progress = progress
        self.stream = stream or sys.stderr
        self.step_wait_seconds = step_wait_seconds
        self.on_step = on_step
        self._has_completed_step = False

    def begin_lane(self) -> None:
        """Reset the step boundary before starting a new benchmark lane."""
        self._has_completed_step = False

    def _wait_for_step(self, description: str) -> None:
        if not self._has_completed_step or self.step_wait_seconds <= 0:
            return
        seconds = self.step_wait_seconds
        print(
            f"\r[wait] next step starts in {seconds:g} s: {description}",
            file=self.stream,
            flush=True,
        )
        time.sleep(seconds)

    def run(
        self,
        command: Sequence[str | Path],
        *,
        description: str,
        cwd: Path | None = None,
        env: Mapping[str, str] | None = None,
        expected_failure: bool = False,
        on_output: Callable[[str, bool], None] | None = None,
    ) -> CommandResult:
        rendered = tuple(str(item) for item in command)
        process_env = os.environ.copy()
        if env:
            process_env.update(env)
        self._wait_for_step(description)
        if self.on_step is not None:
            self.on_step(description)
        progress = _CommandProgress(description, self.stream, self.progress)
        progress.start()
        started = time.perf_counter()
        try:
            if on_output is None:
                completed = subprocess.run(
                    rendered,
                    cwd=cwd,
                    env=process_env,
                    capture_output=True,
                    text=True,
                    check=False,
                )
                stdout = completed.stdout
                stderr = completed.stderr
                returncode = completed.returncode
            else:
                stdout, stderr, returncode = self._run_streamed(
                    rendered,
                    cwd=cwd,
                    env=process_env,
                    on_output=on_output,
                )
        except BaseException:
            progress.finish(False)
            raise
        result = CommandResult(
            command=tuple(_redact(item) for item in rendered),
            elapsed_seconds=time.perf_counter() - started,
            returncode=returncode,
            stdout=stdout,
            stderr=stderr,
        )
        success = (result.returncode == 0) != expected_failure
        progress.finish(success)
        self._has_completed_step = True
        if not success:
            raise CommandError(result, description)
        return result

    @staticmethod
    def _run_streamed(
        rendered: tuple[str, ...],
        *,
        cwd: Path | None,
        env: Mapping[str, str],
        on_output: Callable[[str, bool], None],
    ) -> tuple[str, str, int]:
        process = subprocess.Popen(
            rendered,
            cwd=cwd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        events: queue.Queue[tuple[bool, str | None]] = queue.Queue()

        def read_stream(stream: TextIO, is_stderr: bool) -> None:
            for line in iter(stream.readline, ""):
                events.put((is_stderr, line))
            stream.close()
            events.put((is_stderr, None))

        threads = [
            threading.Thread(
                target=read_stream,
                args=(stream, is_stderr),
                daemon=True,
            )
            for stream, is_stderr in (
                (process.stdout, False),
                (process.stderr, True),
            )
            if stream is not None
        ]
        for thread in threads:
            thread.start()

        stdout_lines: list[str] = []
        stderr_lines: list[str] = []
        closed_streams = 0
        while closed_streams < len(threads):
            is_stderr, line = events.get()
            if line is None:
                closed_streams += 1
                continue
            (stderr_lines if is_stderr else stdout_lines).append(line)
            on_output(line, is_stderr)

        returncode = process.wait()
        for thread in threads:
            thread.join()
        return "".join(stdout_lines), "".join(stderr_lines), returncode
