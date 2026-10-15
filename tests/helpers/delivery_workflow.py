"""Shared correctness operations for HTTP and S3 delivery scenarios."""

from tests.datasets.generate import delivery
from .delivery import fetch_arguments
from .filesystem import repository_inventory as inventory


def published(case):
    data = delivery(case.payload / "source")
    repo = case.init("published")
    captured = case.capture(repo, data.root, "--bundle-preset", "4MiB")
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
    case.stats(
        *fetch_arguments(
            case,
            case.payload / "combined-repo",
            snapshot,
            repo,
            url,
            destination=combined,
        )
    )
    case.compare(data, combined, "combined-fetch")
    (combined / "part-0.bin").write_bytes(b"corrupt")
    case.repair(case.payload / "combined-repo", snapshot, combined)
    case.compare(data, combined, "repair-after-delivery")
