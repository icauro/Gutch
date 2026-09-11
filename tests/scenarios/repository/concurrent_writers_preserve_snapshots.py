"""Operations: Run concurrent CLI captures against one repository.

Expected: Both committed snapshots remain reconstructible with their exact original contents.
"""
import json
from tests.support.cli import start_process, REPOSITORY
from tests.support.datasets import basic
from tests.support.process import stop_process


def run(case):
    repo = case.init()
    datasets = [basic(case.payload / f"source-{i}", i) for i in (1, 2)]
    processes = []
    try:
        for i, data in enumerate(datasets):
            stats = case.root / f"capture-{i}.json"
            process = start_process([str(case.executable), "snapshot", "capture", "--repo", str(repo),
                "--source-dir", str(data.root), "--yes", "--jobs", "2", "--stats-json-out", str(stats)],
                REPOSITORY, case.environment, log_path=case.log("concurrent-capture"))
            processes.append((process, stats, data))
        for i, (process, stats, data) in enumerate(processes):
            assert process.wait(timeout=5) == 0
            snapshot = json.loads(stats.read_text())["snapshot_id"]
            destination = case.payload / f"installed-{i}"
            case.reconstruct(repo, snapshot, destination)
            case.compare(data, destination, f"writer-{i}")
    finally:
        for process, _, _ in processes:
            if process.poll() is None:
                stop_process(process)
