"""Operations: Fetch a snapshot's metadata only, verify an existing copy against it, then fetch fully.

Expected: The metadata-only fetch requests no bundles, yet verification of a matching copy
succeeds. A later full fetch downloads the bundles and reconstruction matches.
"""

from tests.helpers.delivery import DeliveryServer, fetch_arguments
from tests.helpers.delivery_workflow import published


def run(case):
    def bundle_requests(server):
        return sum(
            count
            for path, count in server.requests.items()
            if path.startswith("/bundles/")
        )

    data, repo, snapshot = published(case)
    cache = case.payload / "cache-repo"
    with DeliveryServer(repo) as server:
        stats = case.stats(
            *fetch_arguments(case, cache, snapshot, repo, server.url), "--metadata-only"
        )
        assert stats["metadata_only"] is True
        assert bundle_requests(server) == 0
        assert not (cache / "bundles").exists() or not any(
            path.is_file() for path in (cache / "bundles").rglob("*")
        )
        case.verify(cache, snapshot, data.root)

        case.command(*fetch_arguments(case, cache, snapshot, repo, server.url))
        assert bundle_requests(server) > 0
        destination = case.payload / "fetched"
        case.reconstruct(cache, snapshot, destination)
        case.compare(data, destination, "full-fetch-after-metadata")
