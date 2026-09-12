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

One engine session has three modes, selected with `--profile`:

| Mode | Collection |
|---|---|
| `off` (default) | No diagnostic timers, arguments, or events in operation paths. Essential counters, progress, retries, and cancellation remain active. |
| `summary` | Operation and major-phase inclusive elapsed times. |
| `detailed` | Summary plus sampled worker jobs, diagnostic timings, and progress counters. |

`--profile-out <absolute-directory>` is required for detailed, optional for
summary, and rejected for off. It contains `summary.json` and, in detailed mode,
Perfetto-compatible `trace.json`. `--stats-json-out` remains the operation result;
uncollected diagnostic timing/rate fields are null. Transfer wall rates remain
available independently. The public API exposes `GutchProfileMode` and
`GutchProfiler`, attached through `GutchOperationControl::with_profiler`.
Finish the session after operation controls and worker spans have been dropped.

Events use complete durations (`X`), named worker tracks, and live byte/rate
counters (`C`). File and bundle jobs are sampled at most once per 250 ms per
worker and marker; buffer-read and target-write loops do not emit
individual events; source-chunk jobs receive only time-window samples. Sampling continues throughout the operation. Suppressed jobs
are counted, and sampled duration sums are explicitly marked: they are not total
worker time. Nested inclusive spans must never be added to obtain elapsed time.
`installation.bundle.decode_verify_and_write` includes decode, integrity checks,
and output callbacks; it is not a measurement of decoder CPU time alone.

Thread-local recorders retain bounded typed fields and static names. Recording
has a 64 MiB capacity budget, including recorder bookkeeping, with 1 MiB reserved
for overview events and a separate 100,000 detail-event ceiling. Arguments are
built only after admission. Export streams JSON without cloning or sorting all
events. Reports include omissions, truncation, operation outcome, and trace
export status/time. These bounds concern profiler recording, not total process
memory or transient allocations inside an admitted argument closure.

## Live feedback

Attaching a progress sink enables one coalescing reporter, independently of
profiling. Snapshots include phase completion counts plus download and output
bytes, average bytes/second, recent bytes/second, and stage completion. The CLI
renders the same rates in MiB/s and retains final lines when redirected. The app
adapter translates them to app-owned DTOs; widget presentation is host-owned.

Download bytes count response traffic, including retries. Output bytes count
successful installation writes, excluding artifact staging and skipped files.
They measure writes accepted by the OS, not physical disk throughput or durability.
Averages use stage wall time, not summed worker time; recent rates use an
approximately two-second bounded window and decay during stalls. Updates coalesce
at about 250 ms, with phase/terminal transitions, and final averages freeze.
No sink means no reporter thread. Consumers receive only the latest pending
snapshot; callbacks run outside data workers. The operation result remains the
authority for success or failure.

## Correctness and Benchmark Datasets

[Correctness testing](13_correctness_testing.md) uses the shared immutable
[dataset corpus](../../tests/datasets/README.md). Benchmarks can select the same
case with `--dataset` after its source has been prepared. Both consumers own their
working copies and result directories. Normal tests never download Blender.

```sh
python gutch.py dataset prepare 02_chunk_boundaries
python gutch.py benchmark --dataset 02_chunk_boundaries
```

## Benchmark Execution

Python benchmark fixtures separate tool selection from workload and Gutch
profile configuration. Local and S3 cycle workflows share a scheduler that
runs warmups, rotates repeated cases, and retains partial results and payloads
on failure. Benchmark operation mechanics live in
`tests/benchmarks/support/`.

Dataset inventories determine numeric version order, selected input size,
and a reproducible fingerprint before execution. Reconstruction is checked
before and after recovery, including empty directories, symlink targets, and
supported file permissions. Command measurements exclude cooldown. Gutch
embedded verification and incremental repair remain distinct from external
repository checks and full-restore recovery.

External-tool comparison fixtures select `profile_mode: "off"` to suppress
Gutch diagnostic overhead. Bulk lanes capture and validate the same whole-product tree,
including root files and hidden directories. Restic restores enable content
verification. Local repository size means logical file bytes sampled after
capture; filesystem copy has a separate timing metric. Compression, protection,
and recovery differences are documented in the [benchmark README](../../tests/benchmarks/README.md).

`profile_mode` selects `off`, `summary`, or `detailed`; only detailed retains traces.
Completed runs include raw evidence, a status manifest, samples, and summary
statistics. Docker copies the evidence archive to a unique host directory
before successful environment cleanup. OS caches remain uncontrolled.

## Memory evidence

The opt-in `gutch_default_memory_with_corrupt_repair` fixture records streamed
process-memory samples at a requested 2 ms interval beside each command, plus peak summaries and phase
traces. Unsupported counters remain null. These measurements include mapped
pages and worker stacks; they are not allocation accounting. Sampling and traces
are disabled in off and summary modes. See the [benchmark instructions](../../tests/benchmarks/README.md#process-memory-profiling).

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

## Binary metadata measurements

Run `python tests/benchmarks/binary_layout_experiment.py --repetitions 7` from
the Gutch root to compare production column-group codecs against a whole-group
baseline. Its native measurement code lives in
`tests/benchmarks/binary_layout_native.rs`, compiled only into Rust tests.
The driver builds release tests, measures warm encode/decode and map
lookups, samples process RSS, and records file-read timings. Linux also requests
page-cache eviction for a separate I/O measurement; hardware caches are not
controlled. Results live in `tests/.artifacts/binary-layouts/`. The comparison
process holds multiple representations, so its RSS is not an isolated decoder
allocation measurement. There are no performance pass/fail thresholds.
