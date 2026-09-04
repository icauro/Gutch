"""Parallel S3 upload/download workload with explicit phase boundaries."""

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import hashlib
from pathlib import Path
import time

from ..gutch_fixture_runner.s3 import S3Settings, make_client
from .config import StorageTarget


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
    digest = hashlib.sha256(b"hostapp-benchmark-payload").digest()
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
