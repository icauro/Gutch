# Gutch Regressions

Builds the standalone release `gutch-cli`, uses the shared preparation script to materialize
the tracked deterministic 128 MiB recipe beneath
`gutch_repo/test_dataset/.artifacts`, reconstructs and verifies its snapshots, retains raw timing data, and regenerates
`docs/generated/gutch_regression/index.html`.

```pwsh
python gutch_repo/test_regressions/run_all.py
```

The local S3 regression creates an isolated test-owned SeaweedFS environment.
It creates a temporary bucket, runs capture, S3 publication, reconstruction,
corruption and repair, and compares the repaired output:

```pwsh
uv run python gutch_repo/test_regressions/run_s3.py
```

Success removes the owned Docker environment. Failure stops it and retains
bucket contents for inspection through `tests/environment_manage.py`. A nested
run reuses its parent test environment. Development S3 is never used.

Its S3 run artifacts are kept under `.artifacts/s3/` to keep
Windows repository paths short enough for nested Gutch objects.

Temporary state stays under `.artifacts/test/runs/gutch_regression/`; durable reports are written under `docs/generated/gutch_regression/`.
