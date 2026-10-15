"""Operations: Fetch cold and warm, use combined fetch/reconstruct, then corrupt and repair output and a cached bundle.

Expected: Both installations match, repair succeeds, and the warm fetch sends no extra artifact
requests. A corrupted cached bundle is downloaded again and reconstruction matches.
"""

from tests.helpers.delivery import DeliveryServer, fetch_arguments
from tests.helpers.delivery_workflow import published, exercise_delivery


def run(case):
    data, repo, snapshot = published(case)
    with DeliveryServer(repo) as server:
        exercise_delivery(case, data, repo, snapshot, server.url)
        # Two cold repositories fetch each artifact once; the warm fetch adds none.
        for path in server.files:
            if path.startswith("/bundles/"):
                assert server.requests[path] == 2, (
                    f"unexpected artifact requests for {path}"
                )
        cached = case.payload / "cache-repo"
        artifact = next(
            path for path in (cached / "bundles").rglob("*") if path.is_file()
        )
        original = artifact.read_bytes()
        request_path = "/" + artifact.relative_to(cached).as_posix()
        requests_before = server.requests[request_path]
        artifact.write_bytes(b"corrupted cached artifact")
        case.command(*fetch_arguments(case, cached, snapshot, repo, server.url))
        assert artifact.read_bytes() == original
        assert server.requests[request_path] == requests_before + 1
        destination = case.payload / "recovered-cache"
        case.reconstruct(cached, snapshot, destination)
        case.compare(data, destination, "corrupted-cache-recovered")
    alternative = case.init("alternative")
    captured = case.capture(alternative, data.root, "--chunk-preset", "16KiB")
    assert captured["snapshot_id"] == snapshot
    cached = case.payload / "cache-repo"
    blueprint = cached / "snapshots" / snapshot / "snapshot_blueprint.gtcbin"
    before = blueprint.read_bytes()
    assert (
        before
        != (
            alternative / "snapshots" / snapshot / "snapshot_blueprint.gtcbin"
        ).read_bytes()
    )
    with DeliveryServer(alternative) as server:
        case.command(*fetch_arguments(case, cached, snapshot, alternative, server.url))
        assert blueprint.read_bytes() == before
        assert not any(
            count
            for path, count in server.requests.items()
            if path.startswith("/bundles/")
        )
        destination = case.payload / "retained-layout"
        case.reconstruct(cached, snapshot, destination)
        case.compare(data, destination, "retained-layout")
