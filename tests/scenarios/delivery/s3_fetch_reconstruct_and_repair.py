"""Operations: Publish captured artifacts to isolated S3, fetch cold and warm, reconstruct and
repair, then corrupt a remote artifact and retry after restoring it.

Expected: Delivery and repair match the source; remote corruption preserves the installation
and retry succeeds after the artifact is restored.
"""

import json
from tests.helpers.delivery import publish_to_s3, s3_client, fetch_arguments
from tests.helpers.filesystem import inventory
from tests.helpers.delivery_workflow import published, exercise_delivery


def run(case):
    data, repo, snapshot = published(case)
    url = publish_to_s3(repo, case.root / "s3.json")
    exercise_delivery(case, data, repo, snapshot, url)
    artifact = next(path for path in (repo / "bundles").rglob("*") if path.is_file())
    original = artifact.read_bytes()
    client = s3_client()
    bucket = json.loads((case.root / "s3.json").read_text())["bucket"]
    key = artifact.relative_to(repo).as_posix()
    client.put_object(
        Bucket=bucket, Key=key, Body=bytes(byte ^ 0xFF for byte in original)
    )
    destination = case.payload / "preserved-installation"
    destination.mkdir()
    (destination / "keep.txt").write_bytes(b"existing content")
    before = inventory(destination)
    arguments = fetch_arguments(
        case,
        case.payload / "corrupt-cache",
        snapshot,
        repo,
        url,
        destination=destination,
    )
    case.command(*arguments, success=False)
    assert inventory(destination) == before
    client.put_object(Bucket=bucket, Key=key, Body=original)
    case.stats(*arguments)
    case.compare(data, destination, "restored-s3-artifact")
