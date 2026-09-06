"""Operations: Copy the frozen format fixture and reconstruct and verify all four snapshots.

Expected: Paths, content hashes and supported attributes match the recorded source inventories.
"""
import json
import os
from pathlib import Path
import shutil
from ..support.filesystem import assert_tree


def run(case):
    fixture = Path(__file__).resolve().parents[1] / "datasets/reference-v2"
    expected = json.loads((fixture / "expected.json").read_text())
    repo = case.payload / "repo"
    shutil.copytree(fixture, repo)
    for snapshot, specification in expected.items():
        entries = specification["entries"]
        if os.name == "nt":
            for entry in entries.values():
                if entry["kind"] == "file":
                    entry["executable"] = False
        destination = case.payload / snapshot
        case.reconstruct(repo, snapshot, destination)
        assert_tree(entries, destination, case.root / f"{specification['label']}.json")
        assert case.verify(repo, snapshot, destination)["verification_passed"]
