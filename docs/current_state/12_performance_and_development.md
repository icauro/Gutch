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
repository checks and native restore-over-existing behavior.

`python gutch.py benchmark` lists internal Gutch fixtures. The interactive,
explicit fixture, Docker, reconstruction-storage, and S3 commands remain.
External-tool fixtures and their binary provisioning have been removed from
Gutch; cross-tool comparisons run only in the separate
[comparison suite](../../../comparison_benchmark_suite/README.md).

Internal diagnostics retain chunk/compression/worker profiles, individual versus
batch capture, filesystem baselines, and storage/recovery workloads. Reports
show per-snapshot measurements, totals and throughput; verification and recovery
appear separately. Batch capture does not invent per-snapshot capture timings.
Internal fixture loading, diagnostic reports, and batch/whole-product workflows
are owned by `tests/benchmarks/comparison/`; the separate suite provides shared
process execution, Gutch operations, validation, and measurement helpers.

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

Benchmark corruption leaves permissions unchanged. It flips bytes in a writable
file, deletes another writable file when available, and adds an extra file.

Detailed comparisons report sampled CPU usage (100% per logical core) and peak
RSS for direct tool processes. CPU is weighted by observed command sampling time;
child processes and the harness are excluded. Off-mode and unavailable samples
remain missing values rather than zero utilization.

The comparison suite is a separate root project. Before using Gutch benchmark
commands, install its checkout into the same Python environment:
`uv pip install --python .venv -e <suite-checkout>`. Gutch does not search sibling folders.
Its build image includes the installed suite when available; build and engine
correctness commands do not require it.

## Selectable jump chunking

The borrowed chunk stream uses a statically dispatched `ChunkBoundary` kernel.
FastCDC v2020 level 1 remains the default. Capture accepts `--chunker fastcdc|jc`;
`jc` is the Plakar go-cdc-chunkers v1.1.0 algorithm with its fixed masks and Gear
table. The paper-scaled variant remains test-only. Dispatch happens once per
file; the chunk loop stays statically dispatched. The shifted FastCDC Gear table
is derived at compile time.

The public API exposes `Chunker` and `SnapshotCaptureRequest.chunker`. Repository
profiles and snapshot metadata record `fastcdc-v2020` or `jc-v1.1.0`.
Reconstruction and repair use stored boundaries, with no chunker selection.
Recapturing identical content retains the existing snapshot and its algorithm;
new snapshots use the requested algorithm even for unchanged files. Changing
algorithms can reduce reuse of chunks from older snapshots.

Run `python gutch.py benchmark --fixture 16_gutch_fastcdc_vs_jc --product PATH`
from the Gutch directory. The fixture compares three full-flow runs per algorithm
with FastCDC at 128 KiB and JC at 256 KiB, with identical compression and bundle
settings. Equal target
sizes do not imply equal actual average chunk sizes. Each lane has its own repo.

`tests/benchmarks/cdc_experiment.py` compares the frozen pre-interface adapter,
current FastCDC, Plakar-compatible JC, and paper-scaled JC. It accepts `--base`
and `--updated` build directories, `--name` for a dataset label, and
`--repetitions` (default five). Run from
the project root with `python tests/benchmarks/cdc_experiment.py --base PATH
--updated PATH`. Evidence is written to `tests/.artifacts/jc-experiment/<name>/`, so datasets
do not overwrite each other. The base build is resident during measurement;
updated files are loaded one at a time during untimed reuse analysis.

The experiment reports boundary scanning, scanning plus BLAKE3, streaming plus
BLAKE3, chunk-size statistics, and byte reuse against the previous build.
Input loading is excluded, bytes are resident in memory, and variants rotate
order after a warmup. These are single-thread kernel/pipeline measurements,
not complete backup or capture throughput. Reuse checks chunk contents across
files; it does not estimate compressed repository size.

Plakar compatibility is pinned to go-cdc-chunkers v1.1.0, commit
`b6c4abce19e9691184a0aceefc4e9f53fcfe79c8`. Its fixed cut/jump masks contain 11/10
bits. Paper-scaled JC uses `log2(target)-1` cut bits, one fewer jump bit,
evenly spaced mask positions, and target/2 jumps. Both share the JC loop and
Plakar Gear table; the paper preset is not advertised as an exact upstream
implementation. That table differs from Gutch's and must remain distinct.

The 24 frozen Go-oracle cases in `src/artifact/jc_tests.rs` use xorshift64
(13-left, 7-right, 17-left), seed 917, emitting the low byte after each update.
Each case records its exact size parameters, input length, and cut lengths.
They were generated using the pinned Go library's `jc-v1.1.0` reader API.
The tests run offline. The reference Gear table and JC source retain the
upstream ISC notices; Gutch's FastCDC table retains its upstream MIT notice.

Measured results and limitations: [JC experiment](cdc_experiment.md).
