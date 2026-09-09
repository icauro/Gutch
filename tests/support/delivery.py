"""Controlled HTTP faults and isolated S3 publication for correctness tests."""
from __future__ import annotations

from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import threading
from uuid import uuid4

from .cli import REPOSITORY
from .filesystem import write_json


def repository_files(repo):
    return [path for path in repo.rglob("*") if path.is_file() and
            (path.relative_to(repo).parts[0] in {"snapshots", "bundles", "chunks"} or
             path.name in {"repo_metadata.json", "chunk_map.gtcbin"})]


def fetch_arguments(case, repo, snapshot, published, url, *, destination=None):
    manifest = case.payload / f"urls-{uuid4().hex[:8]}.json"
    write_json(manifest, {p.name: url(p.relative_to(published).as_posix())
                         for p in repository_files(published)
                         if p.relative_to(published).parts[0] in {"bundles", "chunks"}})
    arguments = ["snapshot", "fetch-reconstruct" if destination else "fetch", "--repo", repo,
                 "--snapshot-id", snapshot, "--artifact-url-manifest", manifest,
                 "--transfer-concurrency", "2"]
    for flag, relative in [
        ("repo-metadata-url", "repo_metadata.json"), ("chunk-index-url", "chunk_map.gtcbin"),
        ("metadata-url", f"snapshots/{snapshot}/snapshot_metadata.json"),
        ("blueprint-url", f"snapshots/{snapshot}/snapshot_blueprint.gtcbin"),
    ]:
        arguments += [f"--{flag}", url(relative)]
    if destination:
        arguments += ["--destination", destination, "--jobs", "2"]
    return arguments


class DeliveryServer:
    def __init__(self, repo: Path):
        self.files = {"/" + p.relative_to(repo).as_posix(): p for p in repository_files(repo)}
        self.faults = {}
        self.requests = Counter()
        self.request_started = threading.Event()
        self.release_request = threading.Event()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                owner.requests[self.path] += 1
                fault = owner.faults.get(self.path)
                if fault == "retry_once" and owner.requests[self.path] == 1:
                    self.send_error(503)
                    return
                if self.path not in owner.files or fault == "missing":
                    self.send_error(404)
                    return
                data = owner.files[self.path].read_bytes()
                if fault == "invalid":
                    data = b"invalid metadata"
                if fault == "corrupt":
                    data = bytes(byte ^ 0xFF for byte in data)
                if fault == "hold":
                    owner.request_started.set()
                    owner.release_request.wait(30)
                try:
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data[:len(data)//2] if fault == "truncate" else data)
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                    pass

            def log_message(self, *_):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def url(self, relative):
        return f"http://127.0.0.1:{self.server.server_port}/{relative}"

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.release_request.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


def s3_client():
    import boto3
    from botocore.config import Config
    return boto3.client("s3", endpoint_url=os.environ["GUTCH_TEST_S3_ENDPOINT"],
        aws_access_key_id=os.environ["GUTCH_TEST_S3_ACCESS_KEY"],
        aws_secret_access_key=os.environ["GUTCH_TEST_S3_SECRET_KEY"], region_name="us-east-1",
        config=Config(signature_version="s3v4", s3={"addressing_style": "path"}, connect_timeout=3, read_timeout=3))


def publish_to_s3(repo, evidence):
    client = s3_client()
    bucket = f"gutch-correctness-{uuid4().hex[:16]}"
    # Record ownership before creation so interrupted runs retain useful evidence.
    write_json(evidence, {"project": os.environ["GUTCH_TEST_S3_PROJECT"], "bucket": bucket})
    client.create_bucket(Bucket=bucket)
    for path in repository_files(repo):
        client.put_object(Bucket=bucket, Key=path.relative_to(repo).as_posix(), Body=path.read_bytes())
    return lambda relative: client.generate_presigned_url("get_object", Params={"Bucket": bucket, "Key": relative}, ExpiresIn=900)


def cleanup_s3_publication(evidence):
    """Called in the separate cleanup process only after scenario success."""
    if not evidence.is_file():
        return
    import json
    bucket = json.loads(evidence.read_text())["bucket"]
    client = s3_client()
    for page in client.get_paginator("list_objects_v2").paginate(Bucket=bucket):
        objects = [{"Key": item["Key"]} for item in page.get("Contents", [])]
        if objects:
            result = client.delete_objects(Bucket=bucket, Delete={"Objects": objects})
            if result.get("Errors"):
                raise RuntimeError(f"S3 cleanup failed: {result['Errors']}")
    client.delete_bucket(Bucket=bucket)
