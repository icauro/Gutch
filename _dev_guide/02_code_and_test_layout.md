# Code and test layout

## Source navigation

Start with [the public API](../src/11_api.rs). [Capture](../src/08_capture) owns source
selection through packing; [installation](../src/09_installation) owns inspection,
change planning, execution and publication. [Transfer](../src/05_transfer) supplies
artifact readiness, with separate scheduling, adaptive-policy and feedback owners.
[Repository](../src/07_repo) owns acquisition, guarded readers, persistence, index
recovery and synchronization. Installation coordinates combined fetch and
reconstruction; [artifact](../src/04_artifact) owns the binary codecs. The
[module map](internals/01_overview/01_operation_ownership.md#crate-and-module-map) records dependency boundaries.
Runnable library examples live in `examples/`.

## Where work belongs

| Work | Location |
| --- | --- |
| Engine implementation and unit tests | `src/` |
| Public Rust API contracts | `tests/*.rs` |
| Developer entrypoint: setup, test, fuzz, dataset, benchmark, replay, minimize, clean, build-image | `gutch.py` |
| Correctness runner and suite selection | `tests/run.py` |
| CLI behavior and repository regressions | `tests/scenarios/<behavior>/` |
| Shared repository guarantees across all dataset types | `tests/repository_suite.py` |
| Process control and independent test assertions | `tests/helpers/` |
| Frozen repositories and independent expectations | `tests/datasets/corpus/` |
| Dataset generation and real-source preparation | `tests/datasets/` |
| Tests of Python tools | `tests/harness_tests/` |
| Performance comparisons and storage measurements | `tests/benchmarks/` |
| Developer commands, Docker image and service lifecycle | `_tools/` |

Inside `tests/benchmarks/`:

| Folder | Responsibility |
| --- | --- |
| `support/` | Process execution, run layout, validation, memory, traces, S3 transport and reconstruction diagnostics |
| `scheduling/` | Scenario loading, dataset selection, case scheduling and environment |
| `comparison/` | Gutch fixture catalog and parsing, lane runner, batch and whole-product capture, diagnostic reports, binary building and the S3 cycle |
| `docker/` | Benchmark Linux provisioning and evidence export |
| `s3_throughput/` | Standalone object-storage workload measurements |

## Test boundaries

- Python owns CLI workflows. Rust tests keep algorithm invariants and public API
  contracts that need injected providers, cancellation or internal fault hooks.
- Unit tests need no network; remote paths go through injected providers.
- Correctness tests never import benchmark helpers or consume mutable benchmark
  datasets. Both may select the same immutable dataset corpus.
- Dataset generation under `tests/datasets/` never imports its consumers or
  assertions. Correctness owns independent expectations and generates its
  inputs inside each run.
- Validate reconstruction independently before repair, so repair cannot conceal
  an initial failure.
- Do not update frozen expectations merely to make a failing test pass.

## Scenario files

Each Python scenario is one descriptively named file under
`tests/scenarios/<behavior>/`; its relative path without `.py` is the case
identifier in selection, reports and replay. The file has a `run(case)`
entrypoint and an opening docstring stating its operations and expected outcome.
Register it in `tests/run.py`, keep reusable mechanics in `tests/helpers/`, and
use independent source inventories for content assertions.

## Rust unit test layout

Each source file has at most one test module, named `tests`. Keep it inline
while it stays under about 100 lines; beyond that, move it beside the source as
`<stem>_tests.rs` and declare it with `#[cfg(test)] #[path = "<stem>_tests.rs"] mod tests;`.
Name tests after the behavior they check, without a `test_` prefix. Shared
fixtures such as temporary directories, hashes, cancellation and progress sinks
live in `src/test_util/`; give each test its own `tempfile::tempdir()`.
