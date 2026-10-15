from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

from tests.benchmarks.support.s3 import (
    S3Settings,
    make_artifact_url_manifest,
)


class _FakeS3Client:
    def generate_presigned_url(self, client_method, Params, ExpiresIn):
        return f"https://local.test/{Params['Key']}"


class GutchBenchmarkS3Tests(unittest.TestCase):
    def test_artifact_manifest_handles_two_level_repository_shards(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_dir:
            repo = Path(temporary_dir) / "repo"
            (repo / "bundles" / "04").mkdir(parents=True)
            (repo / "chunks" / "ab").mkdir(parents=True)
            (repo / "bundles" / "04" / "bundle-id").write_bytes(b"bundle")
            (repo / "chunks" / "ab" / "chunk-id").write_bytes(b"chunk")
            (repo / "repo_metadata.json").write_text("{}", encoding="utf-8")
            manifest = repo.parent / "artifact-urls.json"
            client = _FakeS3Client()
            settings = S3Settings(
                "bucket", "http://127.0.0.1:9634", "auto", "key", "secret"
            )

            urls = make_artifact_url_manifest(
                client, settings, repo, "prefix", manifest
            )

            self.assertEqual(set(urls), {"bundle-id", "chunk-id"})
            self.assertEqual(
                set(json.loads(manifest.read_text(encoding="utf-8"))), set(urls)
            )


if __name__ == "__main__":
    unittest.main()
