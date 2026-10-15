# Gutch Performance and Development

Status: Current

Scope: Gutch component current state

## Runtime Planning

Full verification first builds a rooted, no-follow filesystem inventory that
combines safe traversal, expected-entry metadata checks, and extra-file
detection. It then sorts expected files by descending size, with path as the
tie-breaker, and verifies them through an indexed Rayon parallel iterator.
Scheduling uses one file per Rayon task so idle workers can steal remaining
work instead of waiting on fixed file batches.
The [indexed collection contract](https://docs.rs/rayon/1.12.0/rayon/iter/trait.IndexedParallelIterator.html)
preserves result ordering; completion ordering is concurrent. Targeted
post-repair verification continues to resolve only the affected entries.

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

Capture workers write to exclusively borrowed result slots within a joined scope.
Results retain input order without a mutex per artifact. Memory admission stays
on the caller, and admitted workers finish before a worker or admission error
returns to the operation.

Repository pull indexes the provider's snapshot descriptors once and reuses that
lookup for metadata and bundle-source resolution. Bundle bodies stream directly
to a same-directory temporary file, retain the 256 MiB per-artifact transfer
limit, and pass content-identity verification before atomic publication. A
provider-owned byte body is borrowed during staging rather than cloned. Metadata
bodies remain bounded in-memory inputs for decoding. Failed transfers or identity
checks preserve existing cached bundle contents.

Artifact transfer concurrency is independent from reconstruction `jobs`.
Standalone fetch and unified `installation reconstruct --fetch` share a bounded
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
FastCDC v2020 level 1 remains the default. Capture accepts `--chunker fastcdc|jc|vram256`;
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


## RAM and VRAM-256 comparison

`--chunker vram256` selects RAM byte-comparison boundaries. AVX2 is detected at
runtime once per file, with scalar RAM on unsupported CPUs. Both paths use the
shared `ChunkBoundary` interface and produce the same persisted `ram-v1` identity.
The size preset denotes the initial maximum-search window (and effective minimum),
not FastCDC's average-size mask. Maximum chunk size remains four times the preset.
The cutoff byte belongs to the following chunk. Files shorter than the window
form one chunk; a search without a qualifying byte cuts at the maximum or EOF.
Unaligned vector loads stay inside the live input and scalar code handles tails.

Run `python gutch.py benchmark --fixture 17_gutch_fastcdc_jc_vram256 --product PATH`
with a directory containing the builds. The fixture compares FastCDC 128 KiB,
JC 256 KiB, and VRAM-256 with a 128 KiB window, using zstd 3 and 8 MiB bundles.
It runs three repetitions with reconstruction and corruption repair. Targets do
not normalize actual chunk counts or reuse. See the implementation's
[reference and provenance](../../src/artifact/ram/README.md).


## Windows inventory metadata reuse

The capture inventory now consumes directory-entry metadata. WalkDir 2.5.0
already caches this information on Windows when links are not followed; requesting
it through the entry avoids redundant per-path metadata queries. Both independent
source inventories, chunk verification, retry behavior and publication checks
remain enabled. The metadata loop is serial, so adding capture workers did not
remove this cost.

An alternating before/after release-binary test used Blender 5.0.0 and 5.1.0,
FastCDC 4 MiB, no compression, 16 MiB bundles and automatic 12 workers. Inputs
were immutable, repositories fresh, profiling detailed, OS cache uncontrolled.
One warm-up was excluded, followed by seven measured repetitions per binary.
Median capture totals fell from 2.549 to 1.742 seconds (31.7%); source inventory
fell from 0.994 to 0.187 seconds (81.2%). Published artifacts, blueprints and the
chunk map were byte-identical in a before/after two-build repository comparison.
All 320 library tests passed, including source modification, truncation,
addition, removal and permission-change checks. Results are Windows-specific;
other platforms do not receive the same cached-metadata optimization.
Evidence: `tests/.artifacts/inventory-metadata/`.

A follow-up full-flow validation passed independent reconstruction and
corruption-repair checks with the optimized release binary.


## Analytical uncompressed probing and cursor reuse

Uncompressed bundle planning now computes the exact probe size from known chunk
lengths plus bundle metadata overhead. It selects the same prefix of chunks and
preserves the estimate and resulting artifacts, while reporting zero probe-read
bytes. The compression profile is still validated; encoding still loads and
checks the chunks. Compressed-codec probing is unchanged.

Payload readers also track their owned file cursor, avoiding a seek on new
handles at offset zero or contiguous reads. Failed reads invalidate the known
position, forcing a seek on a subsequent load. Source switches, gaps and backward
reads remain supported.

An alternating release-binary comparison on Blender 5.0.0/5.1.0 used FastCDC
4 MiB, no compression, 16 MiB bundles, automatic workers and detailed profiling.
One warm-up was excluded; seven measured repetitions used fresh repositories and
uncontrolled OS cache. Median capture totals were baseline 1.764 s, analytical
probe 1.695 s, and analytical probe plus cursor reuse 1.677 s. Probe work dropped
from about 24.7 ms to approximately 1-2 microseconds per build pair. The combined
capture improvement was about 4.9%; the incremental seek gain was only about 1%,
so its timing benefit should not be treated as a precise universal estimate.
Both candidate variants produced byte-identical artifacts, blueprints and chunk
maps compared with baseline. All 321 library tests passed; coverage includes
probe threshold/metadata accounting, actual-load failures, contiguous/backward
reads, source switches and recovery after a partial read failure.
Evidence: `tests/.artifacts/probe-seek/`.

Full-flow validation also passed independent reconstruction and corruption
repair with both uncompressed and LZ4 bundles on the two Blender builds.


## Directory traversal review, September 2026

Repository status now collects bundle and total artifact counts in one walk.
Push reuses its local status and snapshot summaries between planning and execution
inside the repository write operation. Repository walkers use cached entry types
for regular files and retain target queries for symlinks, including broken links.
Installation inspection, path comparison and artifact listings move owned paths
where possible. Source timestamp discovery uses cached Windows metadata and no
longer sorts entries to compute a maximum. Deterministic artifact traversal and
installation containment checks remain unchanged. WalkDir is not forked.

Final release CLI status measurements used alternating baseline/final binaries,
one warm-up and nine measured repetitions, without concurrent benchmark or
validation jobs. Outputs matched exactly on both inputs:

| Input | Baseline median | Final median | Reduction |
|---|---:|---:|---:|
| Existing two-snapshot Blender repository | 98.72 ms | 83.66 ms | 15.3% |
| Synthetic 10,000 bundles + 10,000 loose chunks | 1092.69 ms | 55.78 ms | 94.9% |

The synthetic entries are empty files used to isolate enumeration and status
accounting; they are not valid payloads for artifact verification. These are
Windows warm-cache results, not cross-platform performance guarantees.
A capture path-move prototype was dropped: three measured alternating Blender
capture pairs after warm-up gave 1.677 s baseline versus 1.777 s candidate, with
variable samples and no demonstrated benefit. Other small allocation changes
remain for simpler ownership; no separate speedup is claimed for them or push.

Validation: 322 library tests passed, two ignored; an added mixed-artifact test
covers bundle/loose totals and ignored filenames. Baseline/final status matched
for file and broken symlinks. Blender 5.0/5.1 capture, independent reconstruction,
and corruption repair passed with uncompressed and LZ4 profiles. Successful
full-flow validation removed its payloads. Diagnostic captures remain in the
ignored evidence directory: `tests/.artifacts/walkdir-review/`.


## Capture and reconstruction follow-up, September 2026

The two-snapshot Blender 5.0/5.1 profile identified output-path preparation as a
substantial reconstruction cost. One uncompressed restore spent 0.649 s of
1.805 s preparing entries, including 0.515 s resolving output paths. Capture's
largest stage was bundle construction; durable sync remained significant.
Worker span totals overlap and must not be added to command wall time.

The retained change checks the existing resolver cache before querying whether
a component exists. Previously even already-validated ancestors incurred that
query for each descendant. Uncached components still undergo containment checks;
missing components are never cached. Cache lifetime and directory creation order
are unchanged. The added regression checks a newly created escaping symlink
beneath a cached parent.

A second prototype buffered bundle output in 64 KiB blocks. It retained hashes,
explicit flushing and durable sync, and produced identical bundle bytes in small
uncompressed, LZ4 and Zstd comparisons. It was discarded because capture did not
improve consistently.

Alternating baseline/candidate full workflows used one warm-up pair followed by
three measured pairs, profiling off, with uncontrolled OS cache and host load.
Medians below total both snapshots. The candidate combined the path-cache change
and the subsequently discarded writer buffer; the writer does not participate
in reconstruction and encoded bundle bytes were unchanged.

| Profile | Operation | Baseline s | Candidate s | Outcome |
|---|---|---:|---:|---|
| None, 4 MiB chunks, 16 MiB bundles | Capture | 1.579 | 1.577 | No demonstrated gain |
| None, 4 MiB chunks, 16 MiB bundles | Reconstruction | 3.268 | 2.304 | 29.5% reduction |
| LZ4 level 0, 1 MiB chunks, 8 MiB bundles | Capture | 1.438 | 1.457 | 1.3% slower |
| LZ4 level 0, 1 MiB chunks, 8 MiB bundles | Reconstruction | 3.301 | 2.445 | 25.9% reduction |

All comparison lanes passed independent reconstruction and corruption-repair
validation. Final library validation passed 323 tests with two ignored. These
are Windows Blender measurements, not universal speedup claims. Evidence,
prototype binaries and alternating samples are under
`tests/.artifacts/capture-reconstruct-review/`; successful workflow runs remove
their large payloads and retain compact reports and combined traces.

After removing buffering, the final binary passed both full workflows again.
This single confirmation run reconstructed the pair in 2.304 s uncompressed and
2.616 s with LZ4; it is not another repeated median. No capture change is retained.


## Reconstruction buffer and handle reuse, single comparison

A single baseline run followed by a single candidate run used Blender 5.0/5.1,
LZ4 level 0, 1 MiB chunk preset and 8 MiB bundles. No warm-up or repetitions were
performed, to limit SSD writes. Both runs independently validated reconstructed
content and corruption repair; repository bytes and file counts matched.

| Operation, both snapshots | Baseline s | Candidate s |
|---|---:|---:|
| Reconstruction | 2.533 | 2.380 |
| Repair | 0.777 | 0.772 |

The observed reconstruction reduction is 6.1%, preliminary rather than a stable
speedup claim. Cache, run order and host/storage variability were uncontrolled;
individual contributions were not isolated. Capture code was unchanged, yet its
elapsed time varied from 1.414 to 1.511 s, illustrating the noise in this sample.

Retained changes: the bundle decoder keeps initialized buffer storage across
smaller chunks and passes only the current chunk's slice to consumers. Existing
peak buffer capacity is unchanged. Output handle caching remembers successful
file initialization across eviction, avoiding repeated sizing on later reopens.
Initialization state belongs to one reconstruction operation, never to another
command. Concurrent first opens may still both initialize, as before; failed
initialization is not remembered, and a missing previously initialized output
fails to reopen rather than being silently recreated.

Library validation passed 324 tests with two ignored. Coverage includes eviction
with preserved output ranges and decoding large/small/larger selected chunks.
Evidence is under `tests/.artifacts/reconstruct-reuse/`. Successful full-flow runs
removed their payloads. No extra comparison runs were performed.


## Source-reuse cursor tracking, single comparison

Source-chunk reads now track the cursor beside the file handle under the same
mutex. Adjacent reads avoid both the position query and seek; other reads seek
explicitly. Position becomes unknown before any fallible seek/read and is restored
only after a complete read. Evicted handles reopen at position zero. State is
operation-local, with unchanged chunk hashing and source validation.

This affects reconstruction with `--reuse-snapshot` and `--reuse-from`, not ordinary
selective repair: repair currently supplies no reuse source. The first attempted
ordinary-workflow baseline failed during reconstruction with Windows error 5
(access denied), before its candidate ran. It supplies no performance evidence;
failed artifacts/logs remain available. The full workflow was not retried.

One targeted baseline/candidate pair reused the existing uncompressed Blender
repository (4 MiB chunks, 16 MiB bundles), reconstructing 5.1 from 5.0 source files.
No recapture, warm-up or repetitions were performed. Both reported 3,041 reused
chunks (including whole-file reuse). The changed source-chunk path read
111,637,740 bytes, based on total artifact-read bytes minus bundle-read bytes.
Elapsed time was 5.926 s baseline and 5.152 s candidate, an observed 13.1%
reduction, not a stable speedup claim: run order, cache and SSD/host state were
uncontrolled. Ordinary repair is not claimed to improve.

All 325 library tests passed with two ignored. The added test covers sequential
and backward reads, partial-read failure followed by file extension/retry, and
concurrent requests. Independent SHA-256 comparison checked every output file
against the target source for both binaries. Evidence and outputs are under
`tests/.artifacts/repair-cursor/`.


## Reconstruction I/O failure diagnostics

Reconstruction I/O errors identify the operation and affected path when opening,
sizing or writing outputs, reading artifacts or reused source ranges, verifying
output files, creating staging directories, or applying permissions. Directory
promotion and recovery rename failures name both source and destination; promotion
lock failures identify the lock path. Shared directory creation also carries path
context. CLI text and JSON error strings display this context.

The original `std::io::Error` remains the error source, preserving its native OS
code and kind. Adding context does not reclassify cancellation, integrity failures
or already-contextual errors, and preserves existing I/O retry classification.
This improves future diagnosis; it does not establish or fix the cause of the
previous baseline Windows error 5. Cleanup and rollback behavior is unchanged.

Validation: 328 library tests passed, two ignored, and the release CLI built.
Focused tests cover native-error preservation, an actual output-open failure,
and a failed staging promotion that restores the old destination and reports
both paths. No benchmark or large reconstruction was rerun. Logs are under
`tests/.artifacts/io-diagnostics/`.


## Streaming bundle capture, October 2026

Bundle encoding now loads and verifies one source chunk at a time through an
artifact-owned loader boundary. The raw group is no longer held until compression
finishes. Admission reserves the largest chunk plus codec working memory;
inventory, plans, codec buffers, and concurrent jobs still contribute to RSS.
Source hash checks, staging cleanup, encoded-size splitting, and durable sync
remain in place. Compression timing excludes loader time; its enclosing detailed
span includes the nested input spans.

On small Windows warm-cache datasets with four workers and default profiles,
three alternating memory pairs measured these median process peak working sets:

| Input | Baseline | Streaming capture | Reduction |
|---|---:|---:|---:|
| Mixed, 36.5 MiB | 76.36 MiB | 45.39 MiB | 40.6% |
| Four large files, 64 MiB | 76.36 MiB | 45.70 MiB | 40.2% |

Native process high-water counters were sampled about every 3 ms. Separate
21-pair timing runs changed capture medians by -2.2% for 1,000 small files,
-3.9% for mixed files, -0.9% for large files, and +1.2% for repeated-content
fan-out. These small timing differences are workload-specific; the retained
benefit is reduced payload memory, not a universal speedup.

All reconstructed output trees matched independent SHA-256 expectations.
Six comparisons also matched every encoded bundle byte and snapshot identity:
small files at 128 KiB and large files at 1 KiB, each with None, Zstd, and LZ4.
Regression tests cover chunk release order, input length errors, late loader
failure cleanup, invalid descriptors, and admission below the whole raw-group
size. Evidence: `tests/.artifacts/research_optimization_pass/round9_report.md`.


## Typed manifest chunk identities, October 2026

Manifest chunk references retain binary `ChunkHash` values instead of separately
allocated hexadecimal strings. Binary decoding constructs those values directly;
JSON decoding validates canonical text at the boundary. Index lookups and source
reuse plans use the typed values. Verification borrows the manifest's chunk slice
instead of allocating and parsing another chunk list.

On Windows warm-cache captures with four workers and 1 KiB chunk targets,
21 alternating pairs on small deterministic inputs measured:

| Input | Baseline capture | Typed chunks | Reduction |
|---|---:|---:|---:|
| Mixed, 36.5 MiB | 444.57 ms | 397.08 ms | 10.7% |
| Four files, 64 MiB | 578.93 ms | 489.48 ms | 15.5% |

On the same 64 MiB repository, 21 pairs reduced verification from
39.60 to 26.72 ms (32.5%) and reconstruction from
322.22 to 262.62 ms (18.5%). Default 128 KiB-profile capture
changes ranged from -1.5% to +0.7%; the demonstrated gain is on high chunk counts.

Three alternating process-memory pairs measured peak working set reductions from
63.42 to 60.43 MiB for mixed capture and 67.89 to 63.15 MiB for large capture.
Native high-water counters were sampled about every 3 ms, independently of timing.
These are workload-specific observations, not cross-platform guarantees.

Reconstructed trees matched independent SHA-256 expectations. Six comparisons
also matched bundle and blueprint bytes across None, Zstd and LZ4 at 128 KiB and
1 KiB presets. JSON tests preserve canonical text and reject malformed or
uppercase hashes. Evidence: `tests/.artifacts/research_optimization_pass/round11_report.md`.


## Capture accounting and reuse-cache follow-up, October 2026

Capture counts required bundles in the artifact set validated before publishing
the snapshot. This fixes fresh captures counting generated bundles twice. The operation-local dedupe cache
retains typed hashes and encoded byte estimates; artifact locations stay in the
index. Build stages borrow this cache without cloning or reparsing its keys.

For the existing four-file 64 MiB input with 1 KiB chunk targets, 21 alternating
warm-cache Windows pairs measured an unchanged recapture as follows:

- Median command time: 235.44 to 170.23 ms (27.7% reduction).
- Median peak working set: 45.78 to 34.66 MiB (24.3% reduction), from three separate alternating memory pairs.

Fresh-capture confirmation at 1 KiB targets reduced mixed-input time by 7.4%
and large-input time by 9.0%. Default-profile timings varied across short runs;
a 21-pair large-input confirmation changed the median by -2.2%. No default-profile
speedup is claimed. Fresh-capture peak working sets were essentially unchanged.
Native process high-water counters were sampled about every 3 ms independently
of timing. These are workload-specific results, not global memory bounds.

A regression covers fresh, unchanged, partially reused, historical/unreferenced,
loose-only, and mixed-storage snapshots. It failed before the fix with two required
bundles for a one-bundle capture. Six byte comparisons still matched bundles,
blueprints and chunk indexes across None, Zstd and LZ4. Evidence:
`tests/.artifacts/research_optimization_pass/round12_report.md`.

## Compression probe source reuse, October 2026

Hash-ordered compression samples can repeatedly switch between source files.
The probe now retains up to eight source readers in opening order, closing the
oldest before opening another. Sample order, source hash verification, codec
settings and final artifact bytes remain unchanged. This cache belongs only to
the probe; encoding workers keep their existing reader ownership.

On Windows with four workers, 21 alternating warm-cache pairs measured fresh
captures with 1 KiB chunk targets:

| Input | Baseline | Bounded readers | Reduction |
|---|---:|---:|---:|
| Mixed, 36.5 MiB | 394.93 ms | 332.55 ms | 15.8% |
| Four files, 64 MiB | 455.71 ms | 309.29 ms | 32.1% |

An independent 21-pair run measured 14.4% and 31.6% reductions respectively.
Default 128 KiB-profile changes ranged from -0.9% to +1.0%; no general default
speedup is claimed. Three separate alternating memory pairs measured peak working
set changes of +0.19 MiB (mixed) and +0.66 MiB (large). These are workload-specific
results. The uncompressed and floor-fit planning paths still skip payload reads.

A separate detailed profile reduced the compression-estimation span from 174 to
34 ms on the four-file input. Later detail events reached the trace cap, so this
profile supports the early-stage mechanism only; timing claims use unprofiled
commands. Six comparisons matched bundle, blueprint and chunk-index bytes across
None, Zstd and LZ4. Restored trees matched independent SHA-256 expectations.
Regression tests cover sample order, source eviction, seeks and hash failures on
reader reuse. Evidence: `tests/.artifacts/research_optimization_pass/round13_report.md`.

## Streaming compression samples, October 2026

Compression probing now determines its sample length before reading payloads,
then sends each verified chunk directly to the existing streaming encoder.
Raw sample retention falls from the complete sample to one chunk. Source-reader
limits, sample order, pledged size, codec finalization and artifact bytes stay
unchanged. Compression still owns only codec mechanics; capture owns source I/O.

On Windows with 1 KiB chunk targets and one worker, seven alternating process
memory pairs measured median peak working sets of 36.58 to 29.72 MiB (18.7%
lower) on mixed 36.5 MiB input, and 39.34 to 35.42 MiB (10.0% lower) on the
four-file 64 MiB input. Native process high-water counters were sampled about
every 3 ms in runs separate from latency measurements. Four-worker savings were
smaller: roughly 0.5 to 0.7 MiB in three pairs, with encoding buffers dominating.
These observations are workload-specific, not total-process memory bounds.

Twenty-one alternating timing pairs showed near-flat 1 KiB capture times:
changes of -1.8%/-0.2% with one worker and -2.3%/-0.8% with four workers for
mixed/large inputs respectively. The retained benefit is lower memory use.
Six comparisons matched bundle, blueprint and chunk-index bytes across
None, Zstd and LZ4; restored files matched independent SHA-256 expectations.
Errors while producing sample bytes propagate instead of returning partial
compressed sizes. Evidence: `tests/.artifacts/research_optimization_pass/round14_report.md`.

## Distinct bundle IDs during index writing, October 2026

The chunk-index writer keeps borrowed canonical bundle IDs in its artifact table
and decodes each distinct ID once. Chunk-reference serialization uses those
validated table keys. Per-entry size checks and the final encoded-index reader
validation still run before atomic publication; on-disk bytes are unchanged.

On the four-file 64 MiB input with 1 KiB chunk targets, four workers and warm
Windows caches, 21 alternating summary-profile pairs reduced the median
index-writing span from 26.24 to 23.15 ms (11.8%). No profile events were dropped.
A separate 41-pair unprofiled comparison changed whole capture from 295.49 to
292.48 ms (1.0%); this does not establish a broad capture speedup. Default-profile
screening was mixed. The demonstrated benefit is confined to index writing.

Six comparisons matched bundle, blueprint and chunk-index bytes across
None, Zstd and LZ4. Tests also reject inconsistent sizes, uppercase, non-hex and
short bundle IDs without replacing the existing index. Two other experiments,
empty-update short-circuiting and sorted-vector batch updates, were discarded
after longer comparisons failed to show useful latency or memory improvements.
Evidence: `tests/.artifacts/research_optimization_pass/round15_report.md`.

## Indexed reconstruction target lists, October 2026

Reconstruction planning stores temporary target lists at their existing job and
chunk insertion positions instead of hashing those numeric positions. Copy/source
jobs keep empty slots. Ranges are flattened before sorting jobs or chunks; the
runtime job graph and content-identity lookup maps remain unchanged.

On the four-file 64 MiB repository with 1 KiB chunks, four workers and warm
Windows caches, 21 alternating unprofiled pairs reduced median reconstruction
from 270.82 to 248.88 ms (8.1%). An independent seven-pair screen measured 6.9%.
Restored trees matched independent SHA-256 expectations after every operation.
Default 128 KiB-profile results were smaller and mixed; 41-pair confirmation
changed tiny/mixed medians by -0.1%/-0.6%. No general default speedup is claimed.
Seven separate process-memory pairs measured 77.59 to 78.30 MiB peak working set;
this change is retained for latency, with no demonstrated process-memory benefit.

A focused regression checks repeated chunks at multiple offsets and files,
multiple chunks per artifact, intervening copy/source jobs, and cached/missing
artifact dependencies. All target ranges are checked against explicit expected
file indexes and offsets. Evidence:
`tests/.artifacts/research_optimization_pass/round16_report.md`.

## Coalesced reconstruction writes, October 2026

Artifact workers combine consecutive verified single-target ranges in one lazy
64 KiB output buffer. Large ranges and multi-target chunks use direct positional
writes. Every artifact job explicitly flushes before success or hash forwarding;
cancellation and flush errors abort the job without writes during destruction.
Memory admission includes the extra 64 KiB per artifact job.

On the four-file 64 MiB repository with 1 KiB chunks, four workers and warm
Windows caches, 21 alternating unprofiled pairs reduced median reconstruction
from 266.30 to 161.03 ms (39.5%). An earlier version buffering fanout as well
measured 37.2% in a separate 21-pair run; the final version avoids those copies.
Every restored tree matched independent SHA-256 expectations.

Default 128 KiB-profile confirmation changed tiny/mixed/large/fanout medians by
-0.4%/-0.9%/-2.2%/+0.6%; no general default speedup is claimed. A 41-pair
high-fanout check (835 unique chunks across 64 identical files) changed the median
by +0.6%. Seven separate high-chunk-count memory pairs measured peak working set
80.93 to 82.08 MiB, so no process-memory reduction is claimed. These are local
workload-specific measurements, not cross-platform guarantees.

Tests cover gaps, backward writes, file switches, capacity, large-write bypass,
direct/buffered transitions, typed cancellation, failed final flushes and hash
forwarding only after successful writes. Evidence:
`tests/.artifacts/research_optimization_pass/round18_report.md`.

## Shared artifact identities and derived plan sets, October 2026

Internal bundle references retain shared identifier storage from the repository
index. Cloning references no longer copies bundle ID text. Loose-chunk references
retain their existing representation; provider inputs and reported IDs remain
strings. This uses the existing dependency without changing package versions.
Reconstruction also derives its required-artifact set and bundle count once from
the artifact-job map, removing duplicate per-chunk set updates.

On warm-cache Windows reconstruction with four workers, 21 alternating pairs
measured these changes against the previous implementation:

| Small fixture | Baseline | Combined changes | Reduction |
|---|---:|---:|---:|
| Four files, 64 MiB, 1 KiB chunks | 154.32 ms | 144.47 ms | 6.4% |
| 64 identical files, 64 MiB logical, 835 unique 1 KiB chunks | 180.07 ms | 172.10 ms | 4.4% |

The set-derivation change alone measured 3.2%/3.3% reductions respectively.
A separate 41-pair comparison against that version confirmed another 3.1%
from shared bundle IDs. A 21-pair loose-storage check over 1,000 files stayed
flat. Default-profile screens varied; no broad default speedup is claimed.
Every restored tree matched independent SHA-256 expectations. Seven separate
process-memory pairs changed peak working set from 82.57 to 81.75 MiB; no
process-memory benefit is claimed from this small difference.

Required-set regressions cover source-only copying, mixed source/loose reuse,
repeated bundled references, and cached versus missing artifacts. Evidence:
`tests/.artifacts/research_optimization_pass/round19_report.md`.

## Direct index-row decoding, October 2026

The binary reader builds index rows in one preallocated vector instead of using
a binary-search insertion for each row. A checked constructor enforces strict
key order. Duplicate/descending keys remain errors; artifact/ordinal/size checks
and shared artifact records remain part of decoding. General index mutations
retain their existing behavior.

On Windows with warm caches, 41 alternating unprofiled repository-status pairs
on the four-file 64 MiB / 1 KiB-chunk fixture reduced median latency from
52.64 to 46.63 ms (11.4%). Complete status JSON matched the baseline on every
run. A separate 21-pair unchanged-capture comparison reduced 166.65 to 157.76 ms
(5.3%). Reconstruction confirmation measured a 6.0% reduction over 21 pairs;
default-profile screens were near flat. These are workload-specific observations.

Twenty-one separate process-memory pairs on the status command reduced peak
commit charge from 16.37 to 14.59 MiB (10.9%); peak working set stayed essentially
flat. Native high-water counters were sampled about every 3 ms. No broad RSS or
cross-platform memory bound is claimed.

A second prototype reserved duplicate-ordinal set capacity. Its incremental
41-pair reconstruction check was near flat and memory differences were small,
so it was discarded. Tests cover empty/singleton imports, strict ordering,
duplicates and an encoded index with descending keys. Evidence:
`tests/.artifacts/research_optimization_pass/round20_report.md`.


## Reuse validated artifact requirements, October 2026

Repository status and artifact verification count required bundles from their
already validated artifact sets. They no longer walk every manifest chunk and
query the index a second time solely to construct bundle IDs. Validation still
checks every chunk reference and raw size before any counts are returned.
Loose chunks contribute to artifact counts but not bundle counts; repeated
references to one bundle count once, including when that bundle is missing.

On warm-cache Windows runs, 41 alternating pairs over the four-file 64 MiB
fixture with 1 KiB chunks reduced median status latency from 49.58 to 38.86 ms
(21.6%) and fast repository verification from 90.66 to 70.89 ms (21.8%). Entire
result JSON matched the baseline on every invocation. These measurements target
metadata-heavy small-chunk workloads; no general throughput claim is made.
Evidence: `tests/.artifacts/research_optimization_pass/round21_report.md`.


## Direct repository verification, October 2026

Integrity verification enumerates committed snapshots through repository inventory
and loads the index once. Each loaded manifest flows through path validation and
artifact verification. It no longer builds a presentation status report or reloads
the same blueprint for separate checks. Repository reader protection and the shared
artifact verification cache span the operation. Metadata identity and every chunk
reference remain validated even for snapshots outside the requested payload scope.

Warm-cache Windows command timings against the previous implementation:

| Fixture | Pairs | Previous fast verify | Direct fast verify | Reduction |
|---|---:|---:|---:|---:|
| Four files, 64 MiB, 1 KiB chunks | 41 | 67.58 ms | 40.00 ms | 40.8% |
| Four files, 64 MiB, default chunks | 41 | 16.19 ms | 14.40 ms | 11.1% |
| 64 identical files, 64 MiB logical, 1 KiB chunks | 41 | 36.38 ms | 22.13 ms | 39.2% |
| 1,000 loose artifacts, 4.13 MiB | 41 | 136.64 ms | 128.59 ms | 5.9% |

Strict verification of the first fixture reduced 145.60 to 120.30 ms (17.4%)
over 21 pairs. Complete result JSON matched the baseline on every timed invocation.
Twenty-one separate process-memory pairs on fast verification reduced median peak
working set from 22.08 to 20.02 MiB and peak commit from 15.32 to 14.43 MiB.
These are workload-specific Windows observations, not general throughput bounds.

Focused regressions cover snapshot selection, missing unselected payloads,
unselected missing/size-mismatched index references, unselected blueprint identity,
empty repositories and unknown selections. Borrowed metadata/blueprint publication
prototypes were discarded after timing confirmations and memory checks showed no
material benefit. Evidence: `tests/.artifacts/research_optimization_pass/round22_report.md`
and `tests/.artifacts/research_optimization_pass/round23_report.md`.


## Capture reuse accounting memory, October 2026

Capture computes reuse counts before publishing updates to its availability map.
This removes the full copy of previously available chunk IDs, while preserving
new/reused counts across fresh, unchanged, modified and mixed-storage captures.
The temporary distinct-required-chunk set is consumed before publication.

On Windows, 21 alternating process-memory pairs for repeated capture of the
four-file 64 MiB / 1 KiB-chunk fixture reduced median peak working set from
34.57 to 32.50 MiB (6.0%) and peak commit from 28.48 to 26.39 MiB (7.3%).
An earlier independent 21-pair comparison also saved about 2 MiB of peak commit.
Fresh-capture process memory stayed essentially flat over 21 pairs.

Command timing differences were within noise: 41 repeated-capture pairs and
21 fresh-capture pairs did not establish a speed change. No latency gain is
claimed. Six fixture/codec comparisons preserved identical artifact, blueprint
and index bytes and snapshot IDs; restored trees matched independent SHA-256
expectations. Existing correctness coverage checks reuse/new counts through
extensions, replacements, missing artifacts and mixed storage modes.

Two additional prototypes were discarded: collecting unchanged-capture required
IDs once, and directly building the availability map. Their incremental latency
checks did not hold up in confirmation, including 81 pairs for the latter.
Evidence: `tests/.artifacts/research_optimization_pass/round24_report.md`.


## Borrow decoded metadata columns, October 2026

Index decoding reads its four fixed-width columns directly from the validated
decoded group instead of first copying them into vectors. Blueprint decoding
borrows explicit chunk hash/size columns and materializes each file's final
chunk records as it visits that file. The returned models still own their data.
Envelope decompression, bounds, key ordering, artifact/ordinal checks, chunk
coverage and final manifest validation remain in their existing owners.

Warm-cache Windows command medians on the four-file 64 MiB / 1 KiB-chunk fixture:

| Operation | Alternating pairs | Baseline | Borrowed columns | Reduction |
|---|---:|---:|---:|---:|
| Repository status | 41 | 37.85 ms | 34.81 ms | 8.0% |
| Fast repository verification | 21 | 42.89 ms | 41.05 ms | 4.3% |
| Strict repository verification | 21 | 130.82 ms | 127.55 ms | 2.5% |
| Reconstruction | 21 | 142.02 ms | 137.62 ms | 3.1% |

Index borrowing alone reduced status latency 5.3%; a separate 41-pair comparison
confirmed another 2.8% from blueprint borrowing. Complete status and repository
verification JSON matched on every timed invocation; every reconstructed tree
matched independent SHA-256 expectations. Default-profile and repeated-capture
controls were near flat; no broad throughput gain is claimed.

Twenty-one separate process-memory pairs on repository status reduced median
peak working set from 20.22 to 18.23 MiB (9.8%) and peak commit from 14.50 to
13.05 MiB (10.0%). These are workload-specific Windows observations.

Regression coverage truncates index groups at every byte and rejects trailing
data. Blueprint coverage mixes distinct explicit sequences with implicit/empty
files, interleaved directories and symlinks, and truncated chunk columns.
Evidence: `tests/.artifacts/research_optimization_pass/round25_report.md`.


## Bound metadata output reservations, October 2026

Metadata Zstd decoding reserves the declared output length only when the complete
group fits within twice the actual encoded input. Highly compressed groups start
with the existing incremental allocation path. This avoids repeated growth for
hash-heavy columns without allocating large buffers from decoded declarations
alone. Decoded-size, stream-read and window limits remain authoritative.

On the four-file 64 MiB / 1 KiB-chunk fixture, 21 alternating Windows process-memory
pairs reduced median peak commit from 13.08 to 11.38 MiB (13.0%). Peak working set
was near flat. Forty-one unprofiled status pairs reduced 32.70 to 31.93 ms (2.4%);
21 reconstruction pairs reduced 137.77 to 134.39 ms (2.4%). Every status/verification
result matched baseline JSON, and every reconstructed tree matched independent
SHA-256 expectations. Default and loose-storage verification controls were near flat.

An initial partial-reservation prototype increased peak commit on the repeated-file
fixture from 8.45 to 11.52 MiB. Reserving only whole groups removed that regression:
the final 21-pair control stayed near baseline at 8.39 to 8.44 MiB. Six fixture/codec
comparisons retained identical bundle, blueprint and index bytes and snapshot IDs.
These are workload-specific observations, not general memory or throughput bounds.

Single-buffer envelope assembly, encoded-file buffer sizing, and explicit path
capacity were also tested and discarded after comparisons failed to establish
consistent benefits. Regression tests cover raw, mildly and strongly compressed
groups, maximum decoded-length declarations, corruption and existing framing bounds.
Evidence: `tests/.artifacts/research_optimization_pass/round26_report.md`.
