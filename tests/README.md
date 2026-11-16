# Find and run a test

From the Gutch root, run `python gutch.py test`. It runs tooling tests, Rust tests
and every local CLI scenario. First-time compilation can take several minutes.

`helpers/` holds reusable CLI, process and assertion helpers. `harness_tests/`
contains runnable tests of the runners, datasets, cleanup and benchmark tooling.

```sh
python gutch.py test --list
python gutch.py test --suite repository
python gutch.py test --seed 123 --steps 8
python gutch.py test --dataset 02_chunk_boundaries --repository-version v1
python gutch.py test --repo /path/to/repository --expected /path/to/expected.json
python gutch.py test --seed 456 --storage loose_chunks --compression lz4 --mutations 20
python gutch.py test --case repository/frozen_snapshots_match_inventories
python gutch.py test --suite s3
python gutch.py replay <case-evidence-directory>
```

`--case` accepts the exact identifier printed by `--list`, including its category.
That identifier is also the scenario's path below `scenarios/`, without `.py`.
Explicit cases skip the Python tooling and Rust passes. Category suites select
local cases; run `--suite s3` or select the S3 case explicitly for Docker delivery.

| Suite | What it runs |
| --- | --- |
| `check` (default) | Python tooling, Rust tests, all local scenarios |
| `fast` | Python tooling, Rust tests, one CLI round trip |
| `local` | All local CLI scenarios |
| `capture`, `repository`, `reconstruction`, `delivery`, `repair` | Local scenarios for that behavior |
| `s3` | Disposable Docker S3 delivery scenario |
| `all` | Tooling, Rust, local and S3 scenarios |

Focused scenarios have a 10-second deadline. The core repository lifecycle has a
60-second default deadline, adjustable with `--timeout`. Compilation and shared
service setup happen outside those deadlines. Failed cases retain inputs, logs,
comparisons and replay instructions. A successful run retains only `run.json`
and `summary.md`; `--keep-artifacts` pins its detailed evidence instead. Automatic
retention keeps the latest 20 unpinned successful summaries. Failed, running,
unfinished and pinned runs are never pruned. `python gutch.py clean` also compacts
older successful correctness runs, but leaves unrelated scratch files untouched.

## One repository suite, many inputs

`repository_suite.py` owns the shared lifecycle: open/list, strict verification,
reconstruction, append, rejected mutation, deletion, index rebuild and repair.
It verifies all surviving snapshots after each mutation. The frozen repository
and seeded generated trees exercise the same checks. `--steps` adds a reproducible
sequence of capture, delete, rebuild, repair and rejected-mutation operations.

`--repo` always copies the selected repository before mutation. Supply an idle
repository outside the output directory; linked repository entries are rejected.
`--expected` accepts snapshot-ID-to-entries JSON covering exactly the selected
snapshots. `--dataset` resolves a corpus case's source inventories through its
major-version snapshot mappings. Without it, the
suite records an initial reconstruction baseline and labels the report
`initial-reconstruction-consistency`; this does not certify original content.
Newly generated snapshots always use independent SHA-256 expected inventories.

Failed selected-repository runs preserve a separate original input copy for
replay. Generated runs record the seed, operation sequence and settings. Replay
uses current source and reports source changes. It does not reset source history.
`python gutch.py minimize <case-directory>` reruns a failed lifecycle with fewer
additional operations and writes the shortest failing sequence it finds.

## Native fuzzing

`python gutch.py fuzz` runs the cargo-bolero targets in `src/tests/engine_fuzz.rs`
for `--seconds` each; `--target` selects one. It needs `cargo-bolero` and runs on
Linux. See [correctness testing](../_dev_guide/05_correctness_testing.md)
for targets, input limits and seeds.

## Datasets and benchmarks

`python gutch.py dataset --help` exposes the deterministic generator, seed,
version and recipe options. Generated products default to `.artifacts/tests/datasets/products/`
inside this directory. Correctness scenarios create their own small inputs per run.
They never consume mutable benchmark products. The default frozen scenario covers
the ten generated corpus cases. Blender is explicit:
`python gutch.py test --dataset blender --steps 0 --timeout 1800` automatically
prepares checksum-pinned source downloads and captures a disposable repository.
Only its recipe and independent expectations are tracked; caches are ignored.
See [dataset corpus](datasets/README.md) for preparation and freezing rules.

`python gutch.py benchmark list` lists benchmark workloads. See the
[benchmark guide](benchmarks/README.md) for configuration and Docker execution.
Correctness makes assertions; benchmarks measure performance after validation.

`--mutations` adds seeded truncation and bit-flip tests for chunk maps, blueprints
and stored artifacts after the lifecycle. Each input is restored before the next
iteration, and the exact triggering bytes survive a crash or timeout. Decoder
errors are accepted; process crashes are failures. This is mutation testing, not
coverage-guided fuzzing or automatic failure minimization. No extra suite folders
are needed for these input strategies.

See [the correctness contract](../_dev_guide/05_correctness_testing.md)
for supported assertions, fixture policy and evidence details.

Progress announces each selected scenario before execution. Commands and scenario
workers report elapsed time and remaining timeout every five seconds while
waiting; completion output records the scenario duration. Log paths identify
where to inspect a quiet command. These heartbeats indicate waiting, not a
measured percentage of work completed.
