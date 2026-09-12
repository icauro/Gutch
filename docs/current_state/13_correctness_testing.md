# Gutch Correctness Testing

Status: Current

The component owns one runner, independent of benchmark code and datasets:

```sh
python gutch.py test
python gutch.py test --suite fast
python gutch.py test --suite local
python gutch.py test --suite repository
python gutch.py test --suite s3
python gutch.py test --suite all
python gutch.py test --case repository/deletion_preserves_shared_artifacts
python gutch.py test --suite all --list
```

## Ownership and Execution

| Suite | Runs | Needs |
|---|---|---|
| `check` (default) | Python tooling tests, Rust tests and all local CLI scenarios | Python, Cargo; loopback networking |
| `fast` | Python tooling tests, Rust tests and a CLI round trip | Python, Cargo |
| `local` | All local CLI scenarios, including controlled HTTP faults | Python, Cargo; loopback networking |
| `s3` | Capture, cold/warm fetch, combined fetch/reconstruct, repair | Python with Gutch dependencies, Cargo, Docker |
| `all` | Python tooling tests, Rust tests and every CLI scenario | All above |

Behavior categories select their local scenarios. The default command needs no Docker.
S3 startup failures fail the run;
the runner does not silently skip S3 when Docker is unavailable.

`--case` selects exact scenarios and skips the Rust pass. The runner builds the
debug CLI once using the configured Cargo cache. It does not accept arbitrary
executables or invoke benchmark measurements. Rust retains algorithm invariants,
provider-policy tests and cancellation contracts that need engine hooks.

Each Python scenario has one descriptively named file under `tests/scenarios/`
and one `run(case)` entrypoint. Its opening docstring explains the operations
and expected outcome. Each `--case` identifier is its path relative to
`tests/scenarios/`, without `.py`; reusable operations belong in `tests/support/`.

Python scenarios cover capture identity and deduplication, compression/storage
modes, explicit and automatic source discovery, version updates and source
reuse, file attributes, links, verification and repair, snapshot cleanup,
missing artifacts, repository-verification exit status, path-comparison reports, staging overlap, malicious paths, failed promotion, transfer
failures, retries, process interruption and concurrent run isolation.

Existing scenarios also verify reconstruction after deleting the source, metadata-only
recapture, reconstruction after rejected capture, and actual chunk reuse before
snapshot deletion. HTTP and S3 delivery reject same-length corrupted artifacts,
preserve the existing installation, and succeed after the artifact is restored.
The promotion-failure check compares the entire prior installation.

A separate batch case confirms that a later tag conflict returns failure while
an earlier completed snapshot remains reconstructable. The Rust capture contract
compares API and CLI content, tags, snapshot identity, and persisted profiles.

Profile scenarios require equal IDs across different chunk counts, compression,
and storage modes. Repair covers changed chunk sizing, and delivery checks reuse
of an equivalent local blueprint. Rust tests cover canonical ordering, identity
fields, invalid blueprint IDs, and remote pull planning.

The promotion-denial scenario uses Windows file sharing. It explicitly skips
on other platforms. Link scenarios skip when the host cannot create links.
Skipped cases remain visible in the report.

## Datasets and Assertions

`tests/datasets/trees.py` generates small fixed and selectable-seed trees. Recipes include
empty files/directories, duplicate content, changing versions, Unicode paths,
permissions, sizes around chunk boundaries and an 8 MiB transfer dataset that
spans multiple bundles. Expectations come from the input bytes using SHA-256;
they are independent of Gutch's manifests and verification implementation.

`tests/fixtures/repositories/reference-v2/` contains a small frozen repository with four
snapshots. Its expected inventory was recorded from original source bytes.
Tests copy the fixture into the run before reconstructing it. Normal test runs
never regenerate expected output. Intentional format changes replace the
fixture and its independently checked expectations together; this is not a
historical compatibility matrix.

`tests/repository_suite.py` runs the same lifecycle for the frozen fixture,
generated data and repositories selected with `--repo`: list, strict verification,
reconstruction of every snapshot, append, rejected mutation, deletion, index
rebuild and installation repair. All surviving snapshots are checked after every
mutation. `--steps` adds seeded operation sequences; `--storage` and `--compression`
select the capture profile. Selected repositories are copied before mutation;
links and inputs containing the output directory are rejected. `--expected`
supplies independent inventories. Without expectations, an observed baseline is
used only to check consistency, and the report explicitly labels that limitation.

`--mutations` adds seeded bit flips and truncations to chunk maps, blueprints and
artifacts. It checks for decoder crashes or hangs and preserves triggering bytes.
This is not coverage-guided fuzzing or automatic minimization.

Every initial reconstruction is compared before corruption or repair. Tree
comparison checks paths, file bytes, empty directories, supported file
attributes and link targets. There are no benchmark performance thresholds.

## Isolation and Evidence

Each run writes `tests/.artifacts/runs/gutch/<timestamp>-<id>/summary.md` and `run.json`.
The terminal also prints the overall result, Rust test status and CLI scenario
totals, including failures, skips, cancellations and cases that did not run.
Each case has its own payload, configuration, command logs, stats, expected
inventory and comparison evidence. Each Python scenario runs in a separate
process with a **10-second deadline** for focused cases, or a configurable
**60-second default** for the core lifecycle, including dataset creation, CLI
operations and assertions. CLI builds, Rust tests and shared S3 startup happen
before these timers. Rust tests retain their separate runner deadline.
Automated commands receive closed stdin, so confirmation prompts cannot wait
for input from the terminal that launched the runner.

On timeout the runner kills the scenario and its CLI descendants, marks it
failed, retains its evidence and continues with the next scenario. Inspect
`scenario.log` and `commands/` to locate the last operation. Successful payload
and S3 bucket cleanup run in another process with their own 10-second deadline;
`cleanup_seconds` is recorded separately. A cleanup timeout also fails the case.
The Gutch runner tears down its Compose services after the suite.

The download interruption test uses a request
barrier instead of a sleep to select its failure point.

Successful runs retain only `run.json` and `summary.md`; `--keep-artifacts` pins
all details. Failure inputs, logs, comparisons and tracebacks remain available.
Automatic retention keeps the latest 20 finalized unpinned successful summaries.
It never prunes failures, active runs, unfinished cleanup or pinned runs.
A failed command, assertion or cleanup makes the run fail.

Each case also contains `replay.json`. `python gutch.py replay <case-directory>`
validates the case identifier and runs it in a fresh workspace, preserving the
original evidence. A SHA-256 fingerprint of the standalone source inventory
detects source changes, including uncommitted edits, without requiring Git or a
containing repository. Replay reports a changed fingerprint; it does not check
out an older revision or reproduce an operating-system schedule. Core lifecycle
replay preserves the selected original input copy, expectations, seed and options.
Automatic minimization is not implemented. `python gutch.py clean` compacts older
successful correctness runs and applies summary retention, preserving failures and
pinned runs. Unmanaged scratch files and shared datasets are outside automatic cleanup.

S3 uses Gutch's `tools/docker/compose.yml` with synthetic credentials, a
run-owned /24 network, a dynamically published loopback port, and a unique
bucket. Final teardown removes containers, volumes, and the network even after
failure. Local case evidence remains available. No host application environment
is loaded or inherited as fixture ownership.

All paths and commands on this page are relative to the Gutch root. Consumers
can invoke this script and pass explicit artifact and JSONL event paths.
Benchmark preparation and scheduling remain under their own component folders.
