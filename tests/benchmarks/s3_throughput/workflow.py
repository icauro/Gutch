"""Parallel S3 upload/download workload with explicit phase boundaries."""

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import hashlib
from pathlib import Path
import time
from typing import Any

from tests.benchmarks.support.s3 import S3Client, S3Settings, make_client
from tests.benchmarks.s3_throughput.config import StorageTarget


@dataclass(frozen=True)
class StoragePhaseResult:
    target: str
    operation: str
    object_count: int
    bytes: int
    elapsed_seconds: float
    mib_per_second: float

    def as_json(self) -> dict[str, object]:
        return {
            "target": self.target,
            "operation": self.operation,
            "object_count": self.object_count,
            "bytes": self.bytes,
            "elapsed_seconds": self.elapsed_seconds,
            "mib_per_second": self.mib_per_second,
        }


def _settings(target: StorageTarget) -> S3Settings:
    settings = S3Settings.from_environment(
        target.env_prefix,
        bucket_name=target.bucket or "",
        endpoint=target.endpoint or "",
        region=target.region,
    )
    if not settings.bucket:
        raise ValueError(f"storage target {target.name} has no bucket")
    return settings


def _payload(path: Path, size_bytes: int) -> None:
    if size_bytes <= 0:
        raise ValueError("payload size must be positive")
    digest = hashlib.sha256(b"gutch-benchmark-payload").digest() * 32768
    with path.open("wb") as handle:
        remaining = size_bytes
        while remaining:
            block = digest[: min(len(digest), remaining)]
            handle.write(block)
            remaining -= len(block)


def _prefix_key(prefix: str, index: int) -> str:
    return f"{prefix.rstrip('/')}/object-{index:04d}.bin"


def run_prepare(
    target: StorageTarget,
    *,
    prefix: str,
    size_bytes: int,
    object_count: int,
    workers: int,
    work_dir: Path,
) -> StoragePhaseResult:
    settings = _settings(target)
    client = make_client(settings)
    payload = work_dir / "payload.bin"
    payload.parent.mkdir(parents=True, exist_ok=True)
    _payload(payload, size_bytes)
    started = time.perf_counter()

    def upload(index: int) -> None:
        client.upload_file(str(payload), settings.bucket, _prefix_key(prefix, index))

    with ThreadPoolExecutor(max_workers=max(1, workers)) as executor:
        list(executor.map(upload, range(object_count)))
    elapsed = time.perf_counter() - started
    return StoragePhaseResult(
        target.name,
        "upload",
        object_count,
        size_bytes * object_count,
        elapsed,
        (size_bytes * object_count) / (1024 * 1024 * elapsed) if elapsed else 0.0,
    )


def run_download(
    target: StorageTarget,
    *,
    prefix: str,
    object_count: int,
    workers: int,
    runs: int,
    output_dir: Path,
) -> list[StoragePhaseResult]:
    settings = _settings(target)
    client = make_client(settings)
    output_dir.mkdir(parents=True, exist_ok=True)
    results = []
    for run_number in range(1, runs + 1):
        destination = output_dir / target.name / f"run-{run_number:03d}"
        destination.mkdir(parents=True, exist_ok=True)
        started = time.perf_counter()

        def download(index: int) -> int:
            path = destination / f"object-{index:04d}.bin"
            client.download_file(settings.bucket, _prefix_key(prefix, index), str(path))
            return path.stat().st_size

        with ThreadPoolExecutor(max_workers=max(1, workers)) as executor:
            sizes = list(executor.map(download, range(object_count)))
        elapsed = time.perf_counter() - started
        total_bytes = sum(sizes)
        results.append(
            StoragePhaseResult(
                target.name,
                "download",
                object_count,
                total_bytes,
                elapsed,
                total_bytes / (1024 * 1024 * elapsed) if elapsed else 0.0,
            )
        )
    return results


def _list_objects(client: S3Client, bucket: str, prefix: str) -> list[dict[str, Any]]:
    objects: list[dict[str, object]] = []
    continuation_token: str | None = None
    while True:
        parameters: dict[str, object] = {"Bucket": bucket, "Prefix": prefix}
        if continuation_token:
            parameters["ContinuationToken"] = continuation_token
        response = client.list_objects_v2(**parameters)
        objects.extend(response.get("Contents", []))
        if not response.get("IsTruncated"):
            return objects
        continuation_token = response.get("NextContinuationToken")
        if not isinstance(continuation_token, str) or not continuation_token:
            raise RuntimeError(
                "S3 object listing was truncated without a continuation token"
            )


def run_delete(target: StorageTarget, *, prefix: str) -> int:
    settings = _settings(target)
    client = make_client(settings)
    objects = _list_objects(client, settings.bucket, prefix.rstrip("/") + "/")
    deleted = 0
    for start in range(0, len(objects), 1000):
        batch = objects[start : start + 1000]
        if not batch:
            continue
        response = client.delete_objects(
            Bucket=settings.bucket,
            Delete={"Objects": [{"Key": item["Key"]} for item in batch], "Quiet": True},
        )
        errors = response.get("Errors", [])
        if errors:
            raise RuntimeError(f"S3 object deletion failed: {errors}")
        deleted += len(batch)
    return deleted


def _list_multipart_uploads(
    client: S3Client, bucket: str, prefix: str | None
) -> list[dict[str, Any]]:
    uploads: list[dict[str, Any]] = []
    key_marker: str | None = None
    upload_id_marker: str | None = None
    while True:
        parameters: dict[str, object] = {"Bucket": bucket}
        if prefix:
            parameters["Prefix"] = prefix
        if key_marker:
            parameters["KeyMarker"] = key_marker
        if upload_id_marker:
            parameters["UploadIdMarker"] = upload_id_marker
        response = client.list_multipart_uploads(**parameters)
        uploads.extend(response.get("Uploads", []))
        if not response.get("IsTruncated"):
            return uploads
        key_marker = response.get("NextKeyMarker")
        upload_id_marker = response.get("NextUploadIdMarker")
        if not isinstance(key_marker, str) or not isinstance(upload_id_marker, str):
            raise RuntimeError(
                "S3 multipart listing was truncated without continuation markers"
            )


def run_abort_multipart_uploads(target: StorageTarget, *, prefix: str | None) -> int:
    settings = _settings(target)
    client = make_client(settings)
    uploads = _list_multipart_uploads(client, settings.bucket, prefix)
    for upload in uploads:
        client.abort_multipart_upload(
            Bucket=settings.bucket,
            Key=upload["Key"],
            UploadId=upload["UploadId"],
        )
    return len(uploads)


def _add_optional(left, right):
    return None if left is None or right is None else left + right
