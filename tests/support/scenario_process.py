"""Isolated scenario execution with hard deadlines and separately bounded cleanup."""
from __future__ import annotations

import importlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import traceback
import unittest

from .cli import start_process
from .cli import Case, REPOSITORY
from .filesystem import remove_owned, write_json

SCENARIO_TIMEOUT_SECONDS = 10
CLEANUP_TIMEOUT_SECONDS = 10


def kill_process_tree(process):
    from .process import stop_process
    stop_process(process)


def run_worker(mode, module, case, environment, timeout):
    process = start_process([sys.executable, "-m", "tests.support.scenario_process", mode, module,
                             str(case.root), str(case.executable)], REPOSITORY,
                            {**environment, "GUTCH_SCENARIO_WORKER": "1"},
                            log_path=case.root / f"{mode}.log")
    try:
        return process.wait(timeout=timeout)
    except BaseException:
        kill_process_tree(process)
        raise


def run_scenario(module, case, environment, *, keep_artifacts=False):
    started = time.monotonic()
    try:
        code = run_worker("scenario", module, case, environment, SCENARIO_TIMEOUT_SECONDS)
        result_path = case.root / "worker-result.json"
        if code != 0 or not result_path.is_file():
            result = {"status": "failed", "reason": f"scenario process exited with {code}; see scenario.log"}
        else:
            result = json.loads(result_path.read_text(encoding="utf-8"))
    except subprocess.TimeoutExpired:
        result = {"status": "failed", "timed_out": True,
                  "reason": f"scenario exceeded {SCENARIO_TIMEOUT_SECONDS} seconds; see scenario.log and commands/"}
    result["seconds"] = round(time.monotonic() - started, 3)
    if result["status"] in {"passed", "skipped"}:
        cleanup_started = time.monotonic()
        try:
            mode = "cleanup-keep-payload" if keep_artifacts else "cleanup"
            code = run_worker(mode, module, case, environment, CLEANUP_TIMEOUT_SECONDS)
            if code != 0:
                result.update(status="failed", reason=f"cleanup exited with {code}; see {mode}.log")
        except subprocess.TimeoutExpired:
            result.update(status="failed", reason=f"cleanup exceeded {CLEANUP_TIMEOUT_SECONDS} seconds")
        result["cleanup_seconds"] = round(time.monotonic() - cleanup_started, 3)
    return result


def main():
    mode, module, root, executable = sys.argv[1:]
    case = Case(Path(root), Path(executable))
    if mode.startswith("cleanup"):
        from .delivery import cleanup_s3_publication
        cleanup_s3_publication(case.root / "s3.json")
        if mode == "cleanup":
            remove_owned(case.payload, case.root)
        return
    try:
        importlib.import_module(module).run(case)
        result = {"status": "passed"}
    except unittest.SkipTest as error:
        result = {"status": "skipped", "reason": str(error)}
    except BaseException as error:
        result = {"status": "failed", "reason": str(error) or f"{type(error).__name__}: see failure.txt"}
        (case.root / "failure.txt").write_text(traceback.format_exc(), encoding="utf-8")
    result["checks"] = case.checks
    write_json(case.root / "worker-result.json", result)


if __name__ == "__main__":
    main()
