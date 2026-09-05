# Gutch Regressions

Builds the standalone release `gutch-cli`, uses the shared preparation script to materialize
the tracked deterministic 128 MiB recipe beneath
`gutch_repo/test_dataset/.artifacts`, reconstructs and verifies its snapshots, retains raw timing data, and regenerates
`docs/generated/gutch_regression/index.html`.

```pwsh
python gutch_repo/test_regressions/run_all.py
```

The local S3 regression uses the SeaweedFS service from the development Docker
stack. It creates a temporary bucket, runs capture, S3 publication, fetch and
reconstruct, corrupts and repairs each reconstructed snapshot, compares the
repaired output, and deletes the bucket in all completion paths:

```pwsh
python gutch_repo/test_regressions/run_s3.py
```

It uses only the local endpoint (`127.0.0.1`, `localhost`, or `::1`) and never
uses the configured main bucket.

Its S3 run artifacts are kept under `.artifacts/s3/` to keep
Windows repository paths short enough for nested Gutch objects.

Temporary state stays under `.artifacts/test/runs/gutch_regression/`; durable reports are written under `docs/generated/gutch_regression/`.
