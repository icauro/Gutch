"""Shared S3 publication and fetch primitives for Gutch benchmarks."""

from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from dataclasses import dataclass
import json
import os
from pathlib import Path
from collections.abc import Mapping
from typing import Any, Protocol
from uuid import uuid4

from .commands import GutchCli
from .models import GutchProfile


@dataclass(frozen=True)
class S3Settings:
    bucket: str
    endpoint: str
    region: str
    access_key_id: str
    secret_access_key: str

    @classmethod
    def from_environment(
        cls,
        prefix: str = "S3_MAIN",
        *,
        defaults: Mapping[str, str] | None = None,
        **overrides: str,
    ) -> "S3Settings":
        fallback = defaults or {}

        def value(name: str) -> str:
            return (
                overrides.get(name)
                or os.environ.get(f"{prefix}_{name.upper()}", "")
                or fallback.get(name, "")
            ).strip()

        return cls(
            bucket=value("bucket_name"),
            endpoint=value("endpoint"),
            region=value("region") or os.environ.get("AWS_DEFAULT_REGION", "auto"),
            access_key_id=value("access_key"),
            secret_access_key=value("secret_key"),
        )


class S3Client(Protocol):
    def create_bucket(self, **kwargs: Any) -> Any: ...

    def upload_file(self, filename: str, bucket: str, key: str) -> Any: ...
    def download_file(self, bucket: str, key: str, filename: str) -> Any: ...
    def list_objects_v2(self, **kwargs: Any) -> dict[str, Any]: ...
    def delete_objects(self, **kwargs: Any) -> dict[str, Any]: ...
    def delete_bucket(self, **kwargs: Any) -> Any: ...
    def list_multipart_uploads(self, **kwargs: Any) -> dict[str, Any]: ...
    def abort_multipart_upload(self, **kwargs: Any) -> Any: ...
    def generate_presigned_url(self, client_method: str, Params: dict[str, str], ExpiresIn: int) -> str: ...


def make_client(settings: S3Settings) -> S3Client:
    try:
        import boto3
        from botocore.config import Config
    except ImportError as error:  # pragma: no cover
        raise RuntimeError("S3 benchmarks require boto3") from error
    if not settings.bucket:
        raise ValueError("S3 bucket is required")
    return boto3.session.Session().client(
        "s3",
        endpoint_url=settings.endpoint or None,
        region_name=settings.region,
        aws_access_key_id=settings.access_key_id or None,
        aws_secret_access_key=settings.secret_access_key or None,
        config=Config(
            s3={"addressing_style": "path"},
            retries={"max_attempts": 5, "mode": "standard"},
        ),
    )


def _create_bucket(client: S3Client, settings: S3Settings) -> None:
    arguments: dict[str, Any] = {"Bucket": settings.bucket}
    if settings.region and settings.region not in {"auto", "us-east-1"}:
        arguments["CreateBucketConfiguration"] = {"LocationConstraint": settings.region}
    client.create_bucket(**arguments)


def _delete_bucket_contents(client: S3Client, bucket: str) -> None:
    object_request: dict[str, Any] = {"Bucket": bucket}
    while True:
        response = client.list_objects_v2(**object_request)
        objects = response.get("Contents", [])
        if objects:
            client.delete_objects(
                Bucket=bucket,
                Delete={"Objects": [{"Key": item["Key"]} for item in objects]},
            )
        if not response.get("IsTruncated"):
            break
        token = response.get("NextContinuationToken")
        if not token:
            raise RuntimeError(f"S3 returned a truncated listing without a continuation token for {bucket}")
        object_request["ContinuationToken"] = token

    multipart_request: dict[str, Any] = {"Bucket": bucket}
    while True:
        response = client.list_multipart_uploads(**multipart_request)
        for upload in response.get("Uploads", []):
            client.abort_multipart_upload(
                Bucket=bucket,
                Key=upload["Key"],
                UploadId=upload["UploadId"],
            )
        if not response.get("IsTruncated"):
            break
        key_marker = response.get("NextKeyMarker")
        upload_marker = response.get("NextUploadIdMarker")
        if not key_marker or not upload_marker:
            raise RuntimeError(f"S3 returned a truncated multipart listing without markers for {bucket}")
        multipart_request["KeyMarker"] = key_marker
        multipart_request["UploadIdMarker"] = upload_marker


@contextmanager
def temporary_bucket(settings: S3Settings):
    """Create and always remove an isolated bucket using an existing S3 service."""
    bucket = f"hostapp-gutch-regression-{uuid4().hex[:16]}"
    isolated = S3Settings(
        bucket=bucket,
        endpoint=settings.endpoint,
        region=settings.region,
        access_key_id=settings.access_key_id,
        secret_access_key=settings.secret_access_key,
    )
    client = make_client(isolated)
    created = False
    try:
        _create_bucket(client, isolated)
        created = True
        yield isolated
    finally:
        if created:
            _delete_bucket_contents(client, bucket)
            client.delete_bucket(Bucket=bucket)


def repository_files(repo: Path) -> list[Path]:
    return sorted(
        (
            path
            for path in repo.rglob("*")
            if path.is_file() and path.name != "local_repo_config.json"
        ),
        key=lambda path: path.relative_to(repo).as_posix(),
    )


def s3_key(prefix: str, relative: Path) -> str:
    return f"{prefix.strip('/')}/{relative.as_posix()}"


def publish_repository(
    client: S3Client,
    settings: S3Settings,
    repo: Path,
    prefix: str,
    *,
    workers: int = 8,
) -> dict[str, int]:
    files = repository_files(repo)

    def upload(path: Path) -> int:
        client.upload_file(str(path), settings.bucket, s3_key(prefix, path.relative_to(repo)))
        return path.stat().st_size

    with ThreadPoolExecutor(max_workers=max(1, workers)) as executor:
        sizes = list(executor.map(upload, files))
    return {"object_count": len(sizes), "bytes": sum(sizes)}


def make_artifact_url_manifest(
    client: S3Client,
    settings: S3Settings,
    repo: Path,
    prefix: str,
    output: Path,
    *,
    expires_seconds: int = 6 * 60 * 60,
) -> dict[str, str]:
    urls: dict[str, str] = {}
    for path in repository_files(repo):
        relative = path.relative_to(repo)
        parts = relative.parts
        artifact_id = None
        if len(parts) == 3 and parts[0] in {"bundles", "chunks"}:
            artifact_id = parts[2]
        if artifact_id is None:
            continue
        if artifact_id in urls:
            raise ValueError(f"duplicate artifact ID in repository: {artifact_id}")
        urls[artifact_id] = client.generate_presigned_url(
            "get_object",
            Params={"Bucket": settings.bucket, "Key": s3_key(prefix, relative)},
            ExpiresIn=expires_seconds,
        )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(urls, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return urls


def metadata_urls(
    client: S3Client,
    settings: S3Settings,
    repo: Path,
    prefix: str,
    snapshot_id: str,
    *,
    expires_seconds: int = 6 * 60 * 60,
) -> dict[str, str]:
    snapshot = repo / "snapshots" / snapshot_id
    paths = {
        "repo_metadata_url": repo / "repo_metadata.json",
        "metadata_url": snapshot / "snapshot_metadata.json",
        "blueprint_url": snapshot / "snapshot_blueprint.gtcbin",
        "chunk_index_url": repo / "chunk_map.gtcbin",
    }
    product_metadata = snapshot / "snapshot_product_metadata.json"
    if product_metadata.is_file():
        paths["product_metadata_url"] = product_metadata
    urls = {}
    for name, path in paths.items():
        if not path.is_file():
            raise FileNotFoundError(f"missing repository metadata: {path}")
        urls[name] = client.generate_presigned_url(
            "get_object",
            Params={"Bucket": settings.bucket, "Key": s3_key(prefix, path.relative_to(repo))},
            ExpiresIn=expires_seconds,
        )
    return urls


def fetch_command(
    cli: GutchCli,
    repo: Path,
    snapshot_id: str,
    urls: dict[str, str],
    manifest: Path,
    profile: GutchProfile,
    *,
    transfer_concurrency: int,
    transfer_max_concurrency: int | None = None,
    destination: Path | None = None,
    stats_path: Path | None = None,
    trace_path: Path | None = None,
) -> list[str | Path]:
    command = cli.command(
        "snapshot",
        "fetch-reconstruct" if destination is not None else "fetch",
        "--repo",
        repo,
        "--snapshot-id",
        snapshot_id,
        "--repo-metadata-url",
        urls["repo_metadata_url"],
        "--metadata-url",
        urls["metadata_url"],
        "--blueprint-url",
        urls["blueprint_url"],
        "--chunk-index-url",
        urls["chunk_index_url"],
        "--artifact-url-manifest",
        manifest,
        "--transfer-policy",
        "fixed",
        "--transfer-concurrency",
        str(transfer_concurrency),
        "--transfer-max-concurrency",
        str(transfer_max_concurrency or transfer_concurrency),
    )
    if destination is not None:
        if stats_path is None:
            raise ValueError("fetch-reconstruct requires stats_path")
        command.extend(["--destination", destination, *GutchCli.runtime_args(profile), "--stats-json-out", stats_path])
    if trace_path is not None:
        command.extend(["--trace-out", trace_path])
    product_metadata = urls.get("product_metadata_url")
    if product_metadata:
        command.extend(["--product-metadata-url", product_metadata])
    return command
