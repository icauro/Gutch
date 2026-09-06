"""Cold/warm delivery, malformed responses, retries and process interruption."""
from tests.common._process import start_process, stop_process
from ..support.cli import REPOSITORY
from ..support.datasets import delivery
from ..support.delivery import DeliveryServer, fetch_arguments, s3_publication
from ..support.filesystem import inventory


def published(case):
    data = delivery(case.payload / "source")
    repo = case.init("published")
    captured = case.capture(repo, data.root, "--bundle-profile", "bundle_4MiB")
    artifacts = [p for p in (repo / "bundles").rglob("*") if p.is_file()]
    assert len(artifacts) > 1, "delivery scenario must exercise multiple artifacts"
    return data, repo, captured["snapshot_id"]


def exercise_delivery(case, data, repo, snapshot, url):
    cache = case.payload / "cache-repo"
    arguments = fetch_arguments(case, cache, snapshot, repo, url)
    case.command(*arguments)
    destination = case.payload / "fetched"
    case.reconstruct(cache, snapshot, destination)
    case.compare(data, destination, "separate-fetch")
    cached_before = inventory(cache)
    case.command(*arguments)
    assert inventory(cache) == cached_before
    combined = case.payload / "combined"
    case.stats(*fetch_arguments(case, case.payload / "combined-repo", snapshot, repo, url, destination=combined))
    case.compare(data, combined, "combined-fetch")
    (combined / "part-0.bin").write_bytes(b"corrupt")
    case.repair(case.payload / "combined-repo", snapshot, combined)
    case.compare(data, combined, "repair-after-delivery")


def http_delivery(case):
    data, repo, snapshot = published(case)
    with DeliveryServer(repo) as server:
        exercise_delivery(case, data, repo, snapshot, server.url)
        # Two cold repositories fetch each artifact once; the warm fetch adds none.
        for path in server.files:
            if path.startswith("/bundles/"):
                assert server.requests[path] == 2, f"unexpected artifact requests for {path}"


def s3_delivery(case):
    data, repo, snapshot = published(case)
    with s3_publication(repo, case.root / "s3.json") as url:
        exercise_delivery(case, data, repo, snapshot, url)


def failed_fetch_preserves_cache(case):
    data, repo, snapshot = published(case)
    with DeliveryServer(repo) as server:
        cache = case.payload / "cache-repo"
        args = fetch_arguments(case, cache, snapshot, repo, server.url)
        case.command(*args)
        before = inventory(cache)
        server.faults[f"/snapshots/{snapshot}/snapshot_blueprint.gtcbin"] = "invalid"
        case.command(*args, success=False)
        assert inventory(cache) == before
        destination = case.payload / "installed"
        case.reconstruct(cache, snapshot, destination)
        case.compare(data, destination)


def transfer_failures_and_retry(case):
    data, repo, snapshot = published(case)
    with DeliveryServer(repo) as server:
        artifact = next(path for path in server.files if path.startswith("/bundles/"))
        for fault in ("missing", "truncate"):
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


def interrupted_transfer(case):
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
