"""Operations: Run concurrent CLI captures against one repository.

Expected: Both committed snapshots remain reconstructible with their exact original contents.
"""

import json
from tests.helpers.cli import start_process, REPOSITORY
from tests.datasets.generate import basic
from tests.helpers.process import stop_process


def run(case):
    repo = case.init()
    datasets = [basic(case.payload / f"source-{i}", i) for i in (1, 2)]
    processes = []
    try:
        for i, data in enumerate(datasets):
            stats = case.root / f"capture-{i}.json"
            process = start_process(
                [
                    str(case.executable),
                    "snapshot",
                    "capture",
                    "--repo",
                    str(repo),
                    "--source",
                    str(data.root),
                    "--yes",
                    "--jobs",
                    "2",
                    "--json",
                    "--output",
                    str(stats),
                ],
                REPOSITORY,
                case.environment,
                log_path=case.log("concurrent-capture"),
            )
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
