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
When no worker count is supplied, Gutch uses physical CPU count capped by the
logical CPU count available to the process. An explicit worker count overrides
that default but is still capped by available logical parallelism. CLI diagnostics
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

Physical path comparison hashes files with a reusable 1 MiB streaming buffer;
its heap allocation does not grow with the file being compared. Installation
integrity checks share this streaming primitive while retaining their existing
size- and worker-dependent mmap policy for large files.

Codec working-memory reservations are named estimates shared by the encoding
and reconstruction schedulers. Encoding and decoding retain distinct estimates;
the admission budget is not a bound on total process resident memory.

Artifact transfer concurrency is independent from reconstruction `jobs`.
Standalone fetch and unified `snapshot fetch-reconstruct` share a bounded
scheduler with dependency-aware priorities. The default is fixed concurrency
of four; CLI diagnostics and benchmarks can select `--transfer-policy fixed`
or `--transfer-policy adaptive`, with a hard maximum of sixteen. Adaptive
transfers start at the requested initial value. With queued work and stable
positive throughput, they probe one additional worker and retain it only when
averaged throughput improves by at least five percent. Probes use real queued
artifacts rather than extra test requests. Two two-second samples
form an observation; unstable samples do not start a probe. Measurements wait
for the active worker count to match the target, so a still-draining concurrency
change is not mistaken for evidence about the new target.

An unsuccessful probe rolls back one worker and starts a four-second cooldown.
Averaged regressions over ten percent, retries, or downstream reconstruction
backpressure also reduce the target. A stall never increases concurrency; it
aborts an active probe or reduces once after positive progress stops. A draining
queue does not initiate probes. Healthy active downloads are allowed to finish,
and the fixed-four default remains unchanged.

## Profiling Contract

One engine session has three modes, selected with `--profile`:

| Mode | Collection |
|---|---|
| `off` (default) | No diagnostic timers, arguments, or events in operation paths. Essential counters, progress, retries, and cancellation remain active. |
| `summary` | Operation and major-phase inclusive elapsed times. |
| `detailed` | Summary plus consecutive worker jobs, diagnostic timings, and progress counters. |

`--profile-dir <absolute-directory>` is required for detailed, optional for
summary, and rejected for off. It contains `summary.json` and, in detailed mode,
Perfetto-compatible `trace.json`. `--output` remains the operation result;
uncollected diagnostic timing/rate fields are null. Transfer wall rates remain
available independently. The public API exposes `GutchProfileMode` and
`GutchProfiler`, attached through `GutchOperationControl::with_profiler`.
Typed CLI summary builders decide whether diagnostic timing and rate values are
present; renaming a serialized field does not change its profiling visibility.
Finish the session after operation controls and worker spans have been dropped.

Events use complete durations (`X`), named worker tracks, and live byte/rate
counters (`C`). Detailed mode records consecutive file, bundle, and source-chunk
jobs without a time-based sampling interval, until recording limits are reached.
Buffer-read and target-write loops do not emit individual events. Gaps can still
represent waits, uninstrumented work, or omissions after a recording limit;
they are not proof of worker inactivity. Reports count suppressed and dropped
records and mark detail span summaries as sampled when either count is nonzero.
Perfetto also shows a `profile.recording_incomplete` marker with those counts.
Nested inclusive spans must never be added to obtain elapsed time.
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
`comparison_benchmark_suite/support/`. Optional S3 transports remain in
`tests/benchmarks/support/`.

Dataset inventories determine numeric version order, selected input size,
and a reproducible fingerprint before execution. Reconstruction is checked
before and after recovery, including empty directories, symlink targets, and
supported file permissions. Command measurements exclude cooldown. Gutch
embedded verification and incremental repair remain distinct from external
repository checks and full-restore recovery.

The standalone `comparison_benchmark_suite/run.py` ships one Blender fixture
with profiling off, explicit executable resolution, and suite-owned `.artifacts/`.
Its Blender fixture aligns nominal chunks at 1 MiB and selects Zstd level 3 or
the nearest exposed standard preset where supported. The standalone
[suite settings table](../../comparison_benchmark_suite/README.md#comparison-settings)
records exact arguments and the Plakar/BuildPatchTool compression exceptions.
Custom fixtures list build paths in chronological order. Missing tools are
reported without preventing available candidates from running. The existing
developer diagnostic fixtures remain under `tests/benchmarks/comparison/fixtures/`;
those selecting detailed profiling include diagnostic overhead. Bulk lanes capture and validate the same whole-product tree,
including root files and hidden directories. Restic restores enable content
verification. Local repository size means logical file bytes sampled after
capture. Filesystem capture copies the same selected snapshots into storage,
then restore copies them to fresh output. Its retained size includes all copies.
Reports show per-snapshot measurements, totals, per-snapshot averages, and
throughput; verification and enabled recovery appear separately. Compression, protection,
and recovery differences are documented in the [benchmark README](../../tests/benchmarks/README.md).

`profile_mode` selects `off`, `summary`, or `detailed`; only detailed records engine
spans and sampled memory. The comparison timeline includes subprocess wall times
and filesystem-copy operations in every mode, with one zero-based tool/repetition
row and no warm-ups. The `Compacted tool time` track removes gaps between
operations while preserving durations and raw measurement timestamps. Harness
validation, preparation and cleanup are excluded. Detailed engine spans move
with their parent commands and are approximately aligned to command
starts. `open_perfetto` can open this combined overview in every mode.
Completed runs include raw evidence, a status manifest, samples, and summary
statistics. The benchmark's combined `trace.json` preserves measured worker spans,
nesting, and distinct thread identities across commands. Main-thread phases supply
the overview. Bundle-build and reconstruction-job spans enclose detail; reconstruction
jobs report submission-to-start queue delay. Memory-budget and coordinator waits
are measured explicitly. Uninstrumented intervals remain gaps; application spans
are elapsed wall time, not CPU scheduling states. Trace endpoints share the session
clock so microsecond rounding preserves nested boundaries. Docker copies the
evidence archive to a unique host directory before successful environment cleanup. OS caches remain uncontrolled.

## Memory evidence

The opt-in `08_gutch_default_memory_with_corrupt_repair` fixture records streamed
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

Perfetto auto-opening skips empty or metadata-only traces and logs the trace
path instead. Harness tests mock browser opening, including container export
tests, so validation does not launch browser tabs with synthetic traces.
