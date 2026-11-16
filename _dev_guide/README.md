# Developer guide

How to work on Gutch. Read the pages in order for a first change; later, jump
to the one you need. How Gutch itself works is described in the
[documentation index](../_docs/README.md).

| Page | Covers |
| --- | --- |
| [01 Make your first change](01_first_change.md) | Baseline, finding the owner, running a scenario, investigating failures |
| [02 Code and test layout](02_code_and_test_layout.md) | Where work belongs, test boundaries, scenario files, Rust unit tests |
| [03 Folders and outputs](03_folders_and_outputs.md) | Top-level folders, generated output locations, cleanup |

## Related guides

| Guide | Covers |
| --- | --- |
| [Test guide](../tests/README.md) | Suites, options, run evidence and retention |
| [Benchmark guide](../tests/benchmarks/README.md) | Gutch's own diagnostic benchmarks |
| [Dataset guide](../tests/datasets/README.md) | The shared dataset corpus and its generation |
| [Performance and benchmarks](../_docs/06_development/01_performance_and_benchmarks.md) | Runtime planning, profiling, live feedback and benchmark execution |
| [Correctness testing](../_docs/06_development/02_correctness_testing.md) | The correctness runner and what it checks |
| [CLI builds](../_docs/06_development/03_cli_builds.md) | Release builds of `gutch-cli` |
