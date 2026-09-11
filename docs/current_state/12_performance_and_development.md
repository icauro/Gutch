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

In profile mode, the Gutch benchmark runners retain operation traces and print
the same span summary across compared configurations. Performance mode disables
those traces. The regression runner archives
capture, reconstruction, and verification traces beside each snapshot and
uses their root spans for run-level comparison.

## Correctness and Benchmark Datasets

[Correctness testing](13_correctness_testing.md) owns deterministic recipes and
small frozen format fixtures under `tests/`. Each run creates its own
mutable inputs and repositories. It does not use benchmark helpers or datasets.

`test_dataset/` prepares benchmark inputs from tracked recipes,
including optional official Blender builds. These inputs are for workload
measurements; correctness runs do not download Blender or regenerate them.

```sh
python test_dataset/ensure_test_dataset.py
```

## Benchmark Execution

Python benchmark fixtures separate tool selection from workload and Gutch
profile configuration. Local and S3 cycle workflows share a scheduler that
runs warmups, rotates repeated cases, and retains partial results and payloads
on failure. Benchmark operation mechanics live in
`test_benchmarks/support/`.

Dataset inventories determine numeric version order, selected input size,
and a reproducible fingerprint before execution. Reconstruction is checked
before and after recovery, including empty directories, symlink targets, and
supported file permissions. Command measurements exclude cooldown. Gutch
embedded verification and incremental repair remain distinct from external
repository checks and full-restore recovery.

External-tool comparison fixtures select performance mode to suppress Gutch
trace overhead. Bulk lanes capture and validate the same whole-product tree,
including root files and hidden directories. Restic restores enable content
verification. Local repository size means logical file bytes sampled after
capture; filesystem copy has a separate timing metric. Compression, protection,
and recovery differences are documented in the [benchmark README](../../test_benchmarks/README.md).

Performance mode suppresses Gutch trace arguments; profile mode retains traces.
Completed runs include raw evidence, a status manifest, samples, and summary
statistics. Docker copies the evidence archive to a unique host directory
before successful environment cleanup. OS caches remain uncontrolled.

## Memory evidence

The opt-in `gutch_default_memory_with_corrupt_repair` fixture records streamed
process-memory samples at a requested 2 ms interval beside each command, plus peak summaries and phase
traces. Unsupported counters remain null. These measurements include mapped
pages and worker stacks; they are not allocation accounting. Sampling and traces
are disabled in performance mode. See the [benchmark instructions](../../test_benchmarks/README.md#process-memory-profiling).

Manifests, indexes, classifications, and job plans still grow with input size.
The shared installation executor and artifact admission budget do not establish
fixed total process memory for arbitrarily large repositories.

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
