"""Operations: Serve missing, truncated and same-length corrupted artifacts, retry, then inject a single transient HTTP
failure.

Expected: Failed downloads preserve the prior installation and retries produce exact output.
"""
from tests.support.delivery import DeliveryServer, fetch_arguments
from tests.support.filesystem import inventory
from tests.support.delivery_workflow import published


def run(case):
    data, repo, snapshot = published(case)
    with DeliveryServer(repo) as server:
        artifact = next(path for path in server.files if path.startswith("/bundles/"))
        for fault in ("missing", "truncate", "corrupt"):
            server.faults[artifact] = fault
            destination = case.payload / f"install-{fault}"
            destination.mkdir()
            (destination / "keep.txt").write_bytes(b"prior content")
            before = inventory(destination)
            args = fetch_arguments(case, case.payload / f"cache-{fault}", snapshot, repo, server.url, destination=destination)
            case.command(*args, success=False)
            assert inventory(destination) == before
            server.faults.clear()
            case.stats(*args)
            case.compare(data, destination, f"retry-{fault}")
        server.requests.clear()
        server.faults[artifact] = "retry_once"
        destination = case.payload / "retry-once"
        case.stats(*fetch_arguments(case, case.payload / "retry-repo", snapshot, repo, server.url, destination=destination))
        assert server.requests[artifact] >= 2
        case.compare(data, destination, "transient-retry")
