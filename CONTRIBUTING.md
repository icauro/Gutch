# Make your first change

Start with the prerequisites and setup commands in [README.md](README.md).
All commands below run from this directory; no containing project is required.

1. Run `python gutch.py test` to establish a baseline. It runs Python tooling,
   Rust tests and local CLI scenarios. S3 is an explicit additional suite.
2. Find the owner in the [module map](_docs/01_overview/01_operation_ownership.md#crate-and-module-map). Public contracts live in
   `src/11_api.rs`; algorithm tests stay beside the Rust implementation.
3. Find a relevant scenario with `python gutch.py test --list`. A case such as
   `repository/deletion_preserves_shared_artifacts` maps directly to
   `tests/scenarios/repository/deletion_preserves_shared_artifacts.py`.
4. Make the change and run that case with `python gutch.py test --case <case>`.
   For a new CLI behavior, add one scenario file with `run(case)` and an opening
   docstring stating its operations and expected outcome, then register it in
   `tests/run.py`. Use independent source inventories for content assertions.
5. Run the default suite again. Run `python gutch.py test --suite s3` when
   changing remote delivery or service setup. Docker must be running.
6. Update the owning page listed in [`_docs/README.md`](_docs/README.md). Include validation and
   any skipped or unavailable checks in the change description.

## Investigate a failure

The runner prints the case identifier, failure reason, absolute evidence path,
and replay command. Start with `failure.txt`, `scenario.log`, and the comparison
JSON in that directory. Numbered command logs show which operation failed.

Run `python gutch.py replay <case-directory>` to rerun the same scenario in a
fresh workspace. The command reports source changes since the original run.
It does not overwrite the original failure or automatically check out old code.
Repository lifecycle failures also retain the selected original input, independent
expectations when provided, and the seed and operation sequence for generated data.
Timing-dependent failures may need more than one replay; this is scenario replay,
not a recorded operating-system schedule.

## Where work belongs

| Work | Location |
| --- | --- |
| Engine implementation and unit tests | `src/` |
| Public Rust API contracts | `tests/*.rs` |
| CLI behavior and repository regressions | `tests/scenarios/<behavior>/` |
| Shared repository guarantees across all dataset types | `tests/repository_suite.py` |
| Frozen repositories and independent expectations | `tests/datasets/corpus/` |
| Dataset generation and real-source preparation | `tests/datasets/` |
| Performance comparisons and storage measurements | `tests/benchmarks/` |
| Tests of Python tools | `tests/harness_tests/` |
| Process control and independent test assertions | `tests/helpers/` |
| Developer commands, Docker image and service lifecycle | `_tools/` |

Generated files belong under `.artifacts/tests/`; compiler outputs belong under
`.builds/`. Do not update frozen expectations merely to make a failing test pass.

## Rust unit test layout

Each source file has at most one test module, named `tests`. Keep it inline
while it stays under about 100 lines; beyond that, move it beside the source as
`<stem>_tests.rs` and declare it with `#[cfg(test)] #[path = "<stem>_tests.rs"] mod tests;`.
Name tests after the behavior they check, without a `test_` prefix. Shared
fixtures such as temporary directories, hashes, cancellation and progress sinks
live in `src/test_util/`; give each test its own `tempfile::tempdir()`.
