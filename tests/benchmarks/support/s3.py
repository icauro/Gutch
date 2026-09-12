"""Shared S3 publication and fetch primitives for Gutch benchmarks."""

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import json
import os
from pathlib import Path
from collections.abc import Mapping
from typing import Any, Protocol

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
        prefix: str = "GUTCH_S3",
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


def delete_published_prefix(client: S3Client, settings: S3Settings, prefix: str) -> None:
    """Remove a run's newly allocated UUID namespace, never a bucket or caller prefix."""
    import re
    if not re.fullmatch(r".+/[0-9a-f]{32}", prefix):
        raise ValueError("cleanup requires a run-owned UUID prefix")
    prefix = prefix.rstrip("/") + "/"
    while True:
        objects = client.list_objects_v2(Bucket=settings.bucket, Prefix=prefix, MaxKeys=1000).get("Contents", [])
        if not objects:
            return
        keys = [item["Key"] for item in objects]
        if not all(key.startswith(prefix) for key in keys):
            raise ValueError("storage returned an object outside the owned prefix")
        result = client.delete_objects(Bucket=settings.bucket, Delete={"Objects": [{"Key": key} for key in keys]})
        if result.get("Errors"):
            raise RuntimeError(f"S3 cleanup failed for prefix {prefix}: {result['Errors']}")


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
    if trace_path is not None and cli.profile_mode != "off":
        command.extend(["--profile", cli.profile_mode, "--profile-out", trace_path.parent])
    product_metadata = urls.get("product_metadata_url")
    if product_metadata:
        command.extend(["--product-metadata-url", product_metadata])
    return command
