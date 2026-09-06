"""Operations: Warm a repository cache, serve an invalid blueprint, then reconstruct from the
cache.

Expected: Fetch fails without changing cached bytes and the previously cached snapshot remains
usable.
"""
from ..support.delivery import DeliveryServer, fetch_arguments
from ..support.filesystem import inventory
from ..support.delivery_workflow import published


def run(case):
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
