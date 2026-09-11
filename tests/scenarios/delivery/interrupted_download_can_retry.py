"""Operations: Block an artifact request, kill the downloading CLI, release the request and retry.

Expected: The old installation survives interruption and the retry reconstructs exact content.
"""
from tests.support.process import stop_process
from tests.support.cli import start_process
from tests.support.cli import REPOSITORY
from tests.support.delivery import DeliveryServer, fetch_arguments
from tests.support.filesystem import inventory
from tests.support.delivery_workflow import published


def run(case):
    data, repo, snapshot = published(case)
    with DeliveryServer(repo) as server:
        artifact = next(path for path in server.files if path.startswith("/bundles/"))
        server.faults[artifact] = "hold"
        destination = case.payload / "installed"
        destination.mkdir()
        (destination / "keep.txt").write_bytes(b"prior content")
        before = inventory(destination)
        args = fetch_arguments(case, case.payload / "cache-repo", snapshot, repo, server.url, destination=destination)
        process = start_process([str(case.executable), *map(str, args)], REPOSITORY, case.environment, log_path=case.log("interrupted"))
        try:
            assert server.request_started.wait(30), "download did not reach interruption barrier"
        finally:
            stop_process(process)
            server.release_request.set()
        assert process.poll() is not None
        assert inventory(destination) == before
        server.faults.clear()
        case.stats(*args)
        case.compare(data, destination, "retry-after-interruption")
