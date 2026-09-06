# Gutch Correctness Testing

The component owns one runner, independent of benchmark code and datasets:

```sh
uv run python gutch_repo/tests/run_all.py
uv run python gutch_repo/tests/run_all.py --suite local
uv run python gutch_repo/tests/run_all.py --suite s3
uv run python gutch_repo/tests/run_all.py --suite all
uv run python gutch_repo/tests/run_all.py --case snapshot_cleanup
uv run python gutch_repo/tests/run_all.py --suite all --list
```

## Ownership and Execution

| Suite | Runs | Needs |
|---|---|---|
| `fast` (default) | Rust tests and a CLI round trip | Python, Cargo |
| `local` | All local CLI scenarios, including controlled HTTP faults | Python, Cargo; loopback networking |
| `s3` | Capture, cold/warm fetch, combined fetch/reconstruct, repair | Python with repository dependencies, Cargo, Docker |
| `all` | Rust tests and every Python scenario | All above |

`--case` selects exact scenarios and skips the Rust pass. The runner builds the
debug CLI once using the configured Cargo cache. It does not accept arbitrary
executables or invoke benchmark measurements. Rust retains algorithm invariants,
provider-policy tests and cancellation contracts that need engine hooks.

Python scenarios cover capture identity and deduplication, compression/storage
modes, explicit and automatic source discovery, version updates and source
reuse, file attributes, links, verification and repair, snapshot cleanup,
missing artifacts, staging overlap, malicious paths, failed promotion, transfer
failures, retries, process interruption and concurrent run isolation.

The promotion-denial scenario uses Windows file sharing. It explicitly skips
on other platforms. Link scenarios skip when the host cannot create links.
Skipped cases remain visible in the report.

## Datasets and Assertions

`tests/support/datasets.py` generates small fixed-seed trees. Recipes include
empty files/directories, duplicate content, changing versions, Unicode paths,
permissions, sizes around chunk boundaries and an 8 MiB transfer dataset that
spans multiple bundles. Expectations come from the input bytes using SHA-256;
they are independent of Gutch's manifests and verification implementation.

`tests/datasets/reference-v1/` contains a small frozen repository with four
snapshots. Its expected inventory was recorded from original source bytes.
Tests copy the fixture into the run before reconstructing it. Normal test runs
never regenerate expected output. Intentional format changes replace the
fixture and its independently checked expectations together; this is not a
historical compatibility matrix.

Every initial reconstruction is compared before corruption or repair. Tree
comparison checks paths, file bytes, empty directories, supported file
attributes and link targets. Timing is diagnostic only, with no speed thresholds.

## Isolation and Evidence

Each run writes `.artifacts/test/gutch/<timestamp>-<id>/summary.md` and `run.json`.
The terminal also prints the overall result, Rust test status and CLI scenario
totals, including failures, skips, cancellations and cases that did not run.
Each case has its own payload, configuration, command logs, stats, expected
inventory and comparison evidence. Commands have deadlines and terminate their
process tree on interruption. The download interruption test uses a request
barrier instead of a sleep to select its failure point.

Successful case payloads are deleted by default; `--keep-artifacts` retains them.
Failed payloads and tracebacks remain available. Compact logs and comparisons
remain after success. A failed command, assertion or cleanup makes the run fail.

S3 uses the repository's test environment manager with only SeaweedFS enabled
and a unique bucket. Success deletes the bucket and owned Compose resources,
including volumes. Failure retains its environment for inspection. Nested runs
inherit the parent environment's ownership. Development S3 is not used.

Control Center registers local and S3 execution; the pre-deploy Gutch regression
target runs the local suite. The old fixture and regression runners are removed.
Benchmark preparation and scheduling remain under their own component folders.
