"""Operations: Fetch cold and warm, use combined fetch/reconstruct, then corrupt and repair output.

Expected: Both installations match, repair succeeds, and the warm fetch sends no extra artifact
requests.
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
                assert server.requests[path] == 2, f"unexpected artifact requests for {path}"
    alternative = case.init("alternative")
    captured = case.capture(alternative, data.root, "--chunk-profile", "chunk_16KiB")
    assert captured["snapshot_id"] == snapshot
    cached = case.payload / "cache-repo"
    blueprint = cached / "snapshots" / snapshot / "snapshot_blueprint.gtcbin"
    before = blueprint.read_bytes()
    assert before != (alternative / "snapshots" / snapshot / "snapshot_blueprint.gtcbin").read_bytes()
    with DeliveryServer(alternative) as server:
        case.command(*fetch_arguments(case, cached, snapshot, alternative, server.url))
        assert blueprint.read_bytes() == before
        assert not any(count for path, count in server.requests.items() if path.startswith("/bundles/"))
        destination = case.payload / "retained-layout"
        case.reconstruct(cached, snapshot, destination)
        case.compare(data, destination, "retained-layout")
