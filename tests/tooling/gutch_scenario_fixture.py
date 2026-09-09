"""Controlled child behavior for testing the scenario supervisor, never a suite scenario."""
import os
import sys
import time
import unittest

from tests.support.cli import start_process


def run(case):
    behavior = os.environ["GUTCH_FIXTURE_BEHAVIOR"]
    (case.payload / "proof").write_text(behavior)
    if behavior == "fail":
        raise AssertionError("intentional scenario failure")
    if behavior == "skip":
        raise unittest.SkipTest("intentional skip")
    if behavior == "timeout":
        child = start_process([sys.executable, "-c", "import time; time.sleep(60)"],
                              case.root, {}, log_path=case.root / "child.log")
        (case.root / "child.pid").write_text(str(child.pid))
        time.sleep(60)
