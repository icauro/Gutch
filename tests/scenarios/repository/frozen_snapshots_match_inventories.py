"""Operations: Run the shared append/delete/rebuild/repair lifecycle on the frozen repository.

Expected: Paths, content hashes and supported attributes match the recorded source inventories.
"""
import json
from pathlib import Path
from tests.repository_suite import run_lifecycle


def run(case):
    fixture = Path(__file__).resolve().parents[2] / "fixtures/repositories/reference-v2"
    expected = json.loads((fixture / "expected.json").read_text())
    run_lifecycle(case, source=fixture, expected=expected, steps=0)
