# Developer guide

How to work on Gutch. Read the pages in order for a first change; later, jump
to the one you need. How the engine works inside is described in the
[engine internals](internals/README.md); what users see is described in the
[user guide](../_docs/README.md).

| Page | Covers |
| --- | --- |
| [01 Make your first change](01_first_change.md) | Baseline, finding the owner, running a scenario, investigating failures |
| [02 Code and test layout](02_code_and_test_layout.md) | Where work belongs, test boundaries, scenario files, Rust unit tests |
| [03 Folders and outputs](03_folders_and_outputs.md) | Top-level folders, generated output locations, cleanup |
| [04 Performance and benchmarks](04_performance_and_benchmarks.md) | Runtime planning, profiling, live feedback and benchmark execution |
| [05 Correctness testing](05_correctness_testing.md) | The correctness runner, scenarios and the frozen corpus |
| [06 CLI builds](06_cli_builds.md) | Release builds of `gutch-cli` in GitHub Actions |
| [Engine internals](internals/README.md) | Boundaries, stored formats, pipelines and integration contracts |

## Related guides

| Guide | Covers |
| --- | --- |
| [Test guide](../tests/README.md) | Suites, options, run evidence and retention |
| [Benchmark guide](../tests/benchmarks/README.md) | Gutch's own diagnostic benchmarks |
| [Dataset guide](../tests/datasets/README.md) | The shared dataset corpus and its generation |
