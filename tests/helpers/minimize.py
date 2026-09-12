"""Reduce additional lifecycle operations while retaining the original failure evidence."""
import json
from pathlib import Path
import subprocess
import sys
import time
from uuid import uuid4
from tests.helpers.filesystem import write_json
from tests.helpers.cli import execute


def reduce_sequence(sequence, fails):
    """Delete contiguous blocks, then individual operations, while failure persists."""
    sequence = list(sequence)
    width = max(1, len(sequence) // 2)
    while width:
        index = 0
        while index < len(sequence):
            candidate = sequence[:index] + sequence[index + width:]
            if fails(candidate):
                sequence = candidate
            else:
                index += width
        width //= 2
    return sequence


def minimize(directory):
    directory = directory.resolve()
    replay = json.loads((directory / "replay.json").read_text(encoding="utf-8"))
    sequence = json.loads((directory / "sequence.json").read_text(encoding="utf-8"))
    result = json.loads((directory / "results.json").read_text(encoding="utf-8"))
    if result["status"] != "failed":
        raise ValueError("minimize requires a failed lifecycle case")
    def identity(case_directory, record):
        failure = case_directory / "failure.txt"
        frames = [line.strip() for line in failure.read_text(encoding="utf-8").splitlines()
                  if line.lstrip().startswith('File "')] if failure.is_file() else []
        return (record.get("reason", "").replace(str(case_directory), "<case>"),
                frames[-1].replace(str(case_directory), "<case>") if frames else "")
    original_identity = identity(directory, result)
    root = Path(__file__).resolve().parents[2]
    output = root / "tests/.artifacts/tasks" / ("minimize-" + uuid4().hex[:10])
    output.mkdir(parents=True)
    deadline = time.monotonic() + 120
    attempts = 0
    def fails(candidate):
        nonlocal attempts
        if attempts >= 32 or time.monotonic() >= deadline:
            return False
        attempts += 1
        run_id = f"trial-{attempts}"
        arguments = [sys.executable, "gutch.py", "test", "--case", "repository/lifecycle",
                     "--sequence", ",".join(candidate), "--run-id", run_id,
                     "--artifacts-root", str(output)]
        for name in ("repo", "expected", "seed", "storage", "compression", "mutations", "timeout"):
            if name in replay.get("options", {}):
                arguments += ["--" + name, str(replay["options"][name])]
        try:
            execute(arguments, root, {}, output / f"{run_id}.log",
                    timeout=max(1, deadline - time.monotonic()), success=None)
        except subprocess.TimeoutExpired:
            return False
        reports = list(output.glob(f"runs/gutch/{run_id}/cases/repository/lifecycle/results.json"))
        if not reports:
            return False
        report = json.loads(reports[0].read_text(encoding="utf-8"))
        return report["status"] == "failed" and identity(reports[0].parent, report) == original_identity
    if not fails(sequence):
        raise ValueError(f"original failure did not reproduce; evidence: {output}")
    reduced = reduce_sequence(sequence, fails)
    replay.setdefault("options", {})["sequence"] = ",".join(reduced)
    write_json(output / "replay.json", replay)
    write_json(output / "reduction.json", {"original": sequence, "reduced": reduced, "attempts": attempts})
    print(f"Reduced {len(sequence)} operations to {len(reduced)}: {output}")
    return 0
