"""Operations: Fetch cold and warm, use combined fetch/reconstruct, then corrupt and repair output.

Expected: Both installations match, repair succeeds, and the warm fetch sends no extra artifact
requests.
"""
from ..support.delivery import DeliveryServer
from ..support.delivery_workflow import published, exercise_delivery


def run(case):
    data, repo, snapshot = published(case)
    with DeliveryServer(repo) as server:
        exercise_delivery(case, data, repo, snapshot, server.url)
        # Two cold repositories fetch each artifact once; the warm fetch adds none.
        for path in server.files:
            if path.startswith("/bundles/"):
                assert server.requests[path] == 2, f"unexpected artifact requests for {path}"
