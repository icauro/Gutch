# Find and run a test

From the Gutch root, run `python gutch.py test`. It runs tooling tests, Rust tests
and every local CLI scenario. First-time compilation can take several minutes.

```sh
python gutch.py test --list
python gutch.py test --suite repository
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

Scenario setup, execution and assertions have a 10-second deadline; compilation
and shared service setup happen outside that deadline. Failed cases retain logs,
comparisons and payloads. Successful payloads are removed unless
`--keep-artifacts` is supplied. `python gutch.py clean` removes retained successful
payloads under the default run directory, preserving reports and all failed runs.

## Datasets and benchmarks

`python gutch.py dataset --help` exposes the deterministic generator, seed,
version and recipe options. Generated products default to `.artifacts/datasets/products/`
inside this directory. Correctness scenarios create their own small inputs per run.
They never consume mutable benchmark products.

`python gutch.py benchmark list` lists benchmark workloads. See the
[benchmark guide](benchmarks/README.md) for configuration and Docker execution.
Correctness makes assertions; benchmarks measure performance after validation.

The generator is deterministic, but it is not currently a coverage-guided fuzzer.
There are no `generated` or `fuzz` suites yet. Stateful generated tests, failure
minimization and binary-parser fuzzing remain separate planned work; no command
claims to run those checks today.

See [the correctness contract](../docs/current_state/13_correctness_testing.md)
for supported assertions, fixture policy and evidence details.
