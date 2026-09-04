# Gutch Performance and Development

Status: Current

Scope: Gutch component current state

## Runtime Planning

Full verification first builds a rooted, no-follow filesystem inventory that
combines safe traversal, expected-entry metadata checks, and extra-file
detection. It then sorts expected files by descending size and assigns each
successive worker-width batch in alternating directions. This keeps large
files spread across workers while preserving deterministic scheduling and
result ordering. Targeted post-repair verification continues to resolve only
the affected entries.

Build, reconstruction, and verification use the same runtime worker policy.
When no worker count is supplied, Gutch uses the logical CPU count available to
the process. An explicit worker count overrides that default. CLI diagnostics
and benchmarks may override worker count, chunk and bundle sizing, compression
level, and artifact storage mode through the same configuration path rather
than separate implementations.

Bundled and loose-chunk install planning use the same bounded artifact
dependency scheduler. Files become eligible when required artifacts are
available, verified unchanged source-install data may be reused, and staged
files are created lazily on first write. Each file is hash-verified as soon as
all of its producing jobs finish, before promotion. Loose chunks therefore
exercise the same reconstruction scheduling boundary as bundles while exposing
their per-chunk transfer overhead.

Reconstruction output handles are shared through a bounded cache across worker
jobs, and manifest ancestor validation reuses successful checks for the rest of
the operation. Reconstruction timing reports separate prepare, job execution,
verification, finalization, promotion, and total wall phases; worker-time
throughputs remain separate from those wall phases.

Artifact transfer concurrency is independent from reconstruction `jobs`.
Standalone fetch and unified `snapshot fetch-reconstruct` share a bounded
scheduler with dependency-aware priorities. The default is fixed concurrency
of four; CLI diagnostics and benchmarks can select `--transfer-policy fixed`
or `--transfer-policy adaptive`, with a hard maximum of sixteen. Adaptive
transfers start at the requested initial value, ramp up only after stable
throughput improvement, and step down on regressions, retries, or downstream
reconstruction backpressure. Healthy active downloads are allowed to finish.

## Profiling Contract

Gutch supports optional Perfetto-compatible profiling through the global CLI
option `--trace-out <absolute-path>`. The engine carries one optional profiler
through the operation control, so capture, fetch, artifact transfer,
fetch-reconstruct, reconstruction workers, and verification appear in the same
trace. The trace contains nested Chrome JSON begin/end spans and bounded
concurrency-change events without URLs, credentials, or full filesystem paths.

When tracing is enabled, the CLI prints a compact ASCII summary derived from
completed spans and writes the trace after the command finishes. Perfetto is
the detailed timeline and overlap view; the summary is only a quick terminal
view. Verbose snapshot commands print aligned ASCII wall-time and worker-time
tables with MiB/s values. Worker-time rates describe summed work and are not
wall-clock throughput. `--stats-json-out` remains the structured
operation/result report and includes timing/throughput fields for
`fetch-reconstruct`, including artifact transfer bytes, elapsed time, and
average rate. Benchmark scripts use the trace files for overlap analysis
rather than summing legacy timing fields.

The Perfetto process track name is the output trace filename stem, so comparing
`local-reconstruct.json` with `seaweed-fetch-reconstruct.json` shows distinct
process names without exposing the output directory path.

The Gutch benchmark runners retain traces for every Gutch operation and print
the same span summary across compared configurations. The regression runner archives
capture, reconstruction, and verification traces beside each snapshot and
uses their root spans for run-level comparison.

## Checked-In Fixture Regression

Regression test datasets live under `gutch_repo/tests/test_dataset/` and keep the
original source tree beside its generated repo. Source manifests record
expected paths, sizes, and BLAKE3 hashes. Tests reconstruct from stored
repo artifacts and compare against the original tree.

The checked-in cases cover a compact multi-version baseline, append/patch/add/
remove/rename/move behavior, and filesystem behavior including long and
Unicode paths, mixed case, explicit directories, read-only and executable
files, and platform-capable symbolic links. Structured artifacts use `.gtcbin`;
bundle and loose chunk payloads remain extensionless.

Repo format changes regenerate these fixtures as current-format data;
there is no legacy fixture compatibility layer. Automated mutable fixture data
stays beneath `gutch_repo/tests/`. A separate generated 128 MiB recipe
supports regression work, while external product trees are manual benchmark
inputs only.

The focused checked-in regression command is:

```sh
cargo test --manifest-path gutch_repo/Cargo.toml --test fixture_regression
```

## Failure Boundaries

- Performance overrides cannot disable path containment or promotion safety.
- Timing worker values are not added to wall phases as elapsed duration.
- Benchmark-only storage modes do not redefine the current backend product
  delivery format.
- Fixture regeneration replaces expected current-format artifacts and must be
  reviewed with the matching contract change.

For stage-by-stage execution details, see [`04_scan_pipeline.md`](04_scan_pipeline.md),
[`06_reconstruct_pipeline.md`](06_reconstruct_pipeline.md), and
[`07_verify_pipeline.md`](07_verify_pipeline.md).

## Pinned Decisions

- `docs_agentic/pinned_decisions/spec_paths_and_ids.md`
- `docs_agentic/pinned_decisions/spec_quality.md`
