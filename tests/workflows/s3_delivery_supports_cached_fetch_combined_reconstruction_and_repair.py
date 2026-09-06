"""Operations: Publish captured artifacts to isolated S3, fetch cold and warm, reconstruct and
repair.

Expected: Separate and combined delivery match the source and repair restores corrupted bytes.
"""
from ..support.delivery import publish_to_s3
from ..support.delivery_workflow import published, exercise_delivery


def run(case):
    data, repo, snapshot = published(case)
    url = publish_to_s3(repo, case.root / "s3.json")
    exercise_delivery(case, data, repo, snapshot, url)
