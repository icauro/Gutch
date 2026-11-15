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
available independently. The public API exposes `ProfileMode` and
`Profiler`, attached through `OperationControl::with_profiler`.
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
working copies and result directories. Ordinary tests stay offline; explicitly
selecting Blender prepares missing downloads automatically.

```sh
python gutch.py dataset prepare 02_chunk_boundaries
python gutch.py benchmark --dataset 02_chunk_boundaries
```

## Benchmark Execution

Python benchmark fixtures separate tool selection from workload and Gutch
profile configuration. Local and S3 cycle workflows share a scheduler that
runs warmups, rotates repeated cases, and retains partial results and payloads
on failure. Benchmark operation mechanics and optional S3
transports live in `tests/benchmarks/support/`.

Dataset inventories determine numeric version order, selected input size,
and a reproducible fingerprint before execution. Reconstruction is checked
before and after recovery, including empty directories, symlink targets, and
supported file permissions. Command measurements exclude cooldown. Gutch
embedded verification and incremental repair remain distinct from external
repository checks and native restore-over-existing behavior.

`python gutch.py benchmark` lists internal Gutch fixtures. The interactive,
explicit fixture, Docker, reconstruction-storage, and S3 commands remain.
Comparisons with other tools live in [Gutch-Benchmarks](https://github.com/icauro/Gutch-Benchmarks).

Internal diagnostics retain chunk/compression/worker profiles, individual versus
batch capture, filesystem baselines, and storage/recovery workloads. Reports
show per-snapshot measurements, totals and throughput; verification and recovery
appear separately. Batch capture does not invent per-snapshot capture timings.
Internal fixture loading, diagnostic reports, and batch/whole-product workflows
are owned by `tests/benchmarks/comparison/`; `tests/benchmarks/support/` and
`tests/benchmarks/scheduling/` provide process execution, Gutch operations,
validation, scheduling and measurement helpers.

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
controlled. Results live in `.artifacts/tests/binary-layouts/`. The comparison
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
--updated PATH`. Evidence is written to `.artifacts/tests/jc-experiment/<name>/`, so datasets
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

The 24 frozen Go-oracle cases in `src/04_artifact/jc_tests.rs` use xorshift64
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
[reference and provenance](../../src/04_artifact/ram/README.md).


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
Evidence: `.artifacts/tests/inventory-metadata/`.

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
Evidence: `.artifacts/tests/probe-seek/`.

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
ignored evidence directory: `.artifacts/tests/walkdir-review/`.


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
`.artifacts/tests/capture-reconstruct-review/`; successful workflow runs remove
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
Evidence is under `.artifacts/tests/reconstruct-reuse/`. Successful full-flow runs
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
`.artifacts/tests/repair-cursor/`.


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
`.artifacts/tests/io-diagnostics/`.


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
size. Evidence: `.artifacts/tests/research_optimization_pass/round9_report.md`.


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
uppercase hashes. Evidence: `.artifacts/tests/research_optimization_pass/round11_report.md`.


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
`.artifacts/tests/research_optimization_pass/round12_report.md`.

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
reader reuse. Evidence: `.artifacts/tests/research_optimization_pass/round13_report.md`.

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
compressed sizes. Evidence: `.artifacts/tests/research_optimization_pass/round14_report.md`.

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
Evidence: `.artifacts/tests/research_optimization_pass/round15_report.md`.

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
`.artifacts/tests/research_optimization_pass/round16_report.md`.

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
`.artifacts/tests/research_optimization_pass/round18_report.md`.

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
`.artifacts/tests/research_optimization_pass/round19_report.md`.

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
`.artifacts/tests/research_optimization_pass/round20_report.md`.


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
Evidence: `.artifacts/tests/research_optimization_pass/round21_report.md`.


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
material benefit. Evidence: `.artifacts/tests/research_optimization_pass/round22_report.md`
and `.artifacts/tests/research_optimization_pass/round23_report.md`.


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
Evidence: `.artifacts/tests/research_optimization_pass/round24_report.md`.


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
Evidence: `.artifacts/tests/research_optimization_pass/round25_report.md`.


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
Evidence: `.artifacts/tests/research_optimization_pass/round26_report.md`.


## Index accounting by artifact row, October 2026

Index decoding reuses each validated numeric artifact row to calculate its chunk's
share of encoded bytes. It no longer builds a bundle-ID tree and searches that
tree for every bundled chunk. The checked row constructor preserves wire order,
so the borrowed artifact-row column remains aligned with the owned entries.
All artifact/ordinal/raw-size validation and integer rounding remain unchanged.

On the four-file 64 MiB / 1 KiB-chunk fixture, a 41-pair Windows repository-status
confirmation reduced median command time from 37.99 to 36.45 ms (4.1%). An initial
41-pair screen measured a 2.2% reduction. Complete status and repository verification
JSON matched on every timed invocation. Memory remained essentially flat over 21
separate process-memory pairs. Verification, unchanged capture and reconstruction
controls varied; no broader latency or memory improvement is claimed. Every restored
tree matched independent SHA-256 expectations.

A five-row regression interleaves loose storage with two bundles whose hash order,
artifact order and chunk ordinals differ. It checks exact rounded shares and the
unchanged loose-artifact size. Existing sorted-key, malformed-reference, truncation,
trailing-byte and shared-artifact checks remain in place.
Evidence: `.artifacts/tests/research_optimization_pass/round27_report.md`.

The capture-owned estimates below supersede this reader accounting pass.


## Physical index entries and capture estimates, October 2026

Runtime index entries now retain physical metadata without a per-chunk encoded
size estimate. Capture derives its reuse estimates from bundle sizes and indexed
raw totals when needed. Installation records complete encoded size on the artifact
job; chunks retain raw content expectations. Loose-artifact length verification
continues to compare the actual file length with the index declaration before
reading payloads. Index callers and fixtures supply physical artifact sizes.

On the four-file 64 MiB / 1 KiB-chunk fixture, an initial 41-pair Windows status
comparison reduced median latency 2.2%; a separate 41-pair confirmation reduced
31.78 to 31.27 ms (1.6%). Capture and reconstruction controls did not establish a
consistent latency change. Full status/verification JSON matched baseline, and
every reconstructed tree matched independent SHA-256 expectations.

Final native process-memory checks included high-water reads after child exit.
Forty-one status pairs reduced median peak commit from 11.38 to 11.20 MiB (1.6%).
Twenty-one repeated-capture pairs reduced 24.30 to 23.93 MiB (1.5%). Earlier polling
comparisons varied; no broader memory or throughput bound is claimed. Six
fixture/codec comparisons retained identical snapshot IDs and bundle, blueprint
and index bytes.

Accounting regressions now live with capture and cover weighted rounding, partial
indexes, missing bundles and loose artifacts. Installation coverage includes an
incorrect declared loose size and confirms that failed artifacts do not publish
precomputed file hashes. Artifact and ordinal validation remain in the index owner.
Evidence: `.artifacts/tests/research_optimization_pass/round28_report.md`.


## Reuse resolved reconstruction chunk positions, October 2026

Reconstruction planning keys its existing chunk-position map by content hash and
retains both the artifact-job index and chunk position. The first occurrence
resolves the repository index and artifact job; repeated occurrences go directly
to that position. Every reference still validates its raw size and records its
own file index, offset, and missing-artifact dependency. The resolved positions
are scoped to one immutable-index plan and are no longer used after sorting.
Source-file and source-chunk reuse retain their existing paths.

On Windows with warm caches, the small repeated-content fixture has 64 identical
1 MiB files and 835 distinct 1 KiB-preset chunks. A 41-pair confirmation reduced
whole reconstruction median from 177.21 to 172.26 ms (2.8%). After consolidating
size checks, 41 final pairs reduced 175.70 to 172.07 ms (2.1%); the paired median
change was -2.0%, with a bootstrap interval of -3.1% to -1.2%. The initial 21-pair
ratio of medians was flat, so the repeated confirmations matter.

A separate seven-pair summary profile reduced planning from 8.53 to 3.52 ms
(58.7%). Profiling was separate from unprofiled whole-operation comparisons.
The four-file 64 MiB / 53,822-unique-chunk control did not establish a consistent
speed change; no general throughput or process-memory benefit is claimed.
Every reconstruction trial matched independently calculated source SHA-256 maps.

Regression coverage checks repeated references within and across files, bundled
and loose storage, available and missing artifacts, all output offsets, and one
missing dependency per file. Incorrect declared sizes are rejected on the first
occurrence and after a hash has already been resolved. File formats, artifact
verification and publication ordering are unchanged. Evidence:
`.artifacts/tests/research_optimization_pass/round31_report.md`.


## Derive missing dependencies from job targets, October 2026

Reconstruction creates missing-file dependency lists from the completed artifact
jobs rather than maintaining a second view while visiting file chunks. Artifact
availability is checked once per job. Repeated targets are deduplicated by file
index, preserving one dependency per missing artifact and file. File indexes are
sorted to retain the previous ordering. Copied files and source chunks retain
zero artifact dependencies.

A reusable marker buffer is allocated only when a missing artifact is found.
Its length follows the planned file count, and only marked positions are reset
between jobs. Available artifacts do not scan target lists for dependencies.
No per-chunk artifact-identity hashing or per-file dependency sets remain.

A separate seven-pair Windows summary profile on the 64-file / 835-distinct-chunk
repeated-content fixture reduced planning from 3.40 to 2.08 ms (38.7%). Final
unprofiled 41-pair comparisons were flat: repeated-content reconstruction
171.74 to 171.60 ms, and the four-file / 53,822-unique-chunk control 138.15 to
137.66 ms. Paired intervals included zero in both cases. This is a planning-phase
improvement and a consolidation of dependency ownership; no whole-reconstruction
speedup or measured process-memory reduction is claimed.

Every reconstruction trial matched independently calculated source SHA-256 maps.
The expanded regression covers interleaved copied/source/artifact jobs, repeated
targets, two missing artifacts for one file, cached artifacts, canonical file-index
order, and resetting markers between missing jobs. Existing bundled/loose repeated
reference and wrong-size checks remain. Evidence:
`.artifacts/tests/research_optimization_pass/round32_report.md`.


## Keep small shared-output working sets local, October 2026

A worker can retain up to 64 references when the complete plan has at most 64
output files and every output uses shared handles. All references point to file
objects already retained by the 256-entry global cache. This avoids repeated
global mutex and LRU work when a small artifact fans out across many files.
Plans with more output files or any independent handles retain the eight-entry
local bound. Creation, sizing, positional writes and handle-release ordering stay
with their existing owners; the policy performs no additional filesystem scan.

On the 64-identical-file / 835-distinct-chunk Windows fixture, 41 alternating
warm-cache pairs reduced reconstruction median from 165.90 to 162.14 ms (2.3%).
A separate 41-pair confirmation reduced 166.74 to 161.16 ms (3.3%); its paired
median change was -3.1%, with a bootstrap interval of -4.1% to -2.4%.
Every reconstructed tree matched independent source SHA-256 expectations.

The 1,000-file control did not establish a speed change. Unique-content results
varied: an initial 41-pair paired analysis suggested a small slowdown, but an
isolated 81-pair confirmation had a paired interval spanning zero. No consistent
unique-content gain or regression is claimed. Separate 21-pair native memory
checks per fixture kept median peak commit within 1% of baseline; no memory
reduction is claimed. Counters included successful post-exit high-water reads.

Regressions check the 64/65-file boundary, a single independent output among
shared files, worker/producer thresholds, and reuse of identical global file
objects across jobs. Existing range, eviction, cancellation and failed-flush
coverage remains. Evidence:
`.artifacts/tests/research_optimization_pass/round33_report.md`.


## Defer source chunk indexing for whole-file reuse, October 2026

Reconstruction planning first selects matching whole-file copies. It builds the
source chunk-location index only when a target file cannot be copied whole.
When needed, source indexing still searches every verified source file, including
a file already selected for copying: another changed target can reuse its chunks.
Source verification, copy-time hashing, staged-file checks and publication remain
unchanged. The lazy index is local to one plan.

On Windows, a four-file 64 MiB source with 53,822 1 KiB-preset chunks was reused
into a fresh destination. All four files used whole-file copying, with zero
required artifacts. An initial 21-pair comparison reduced median reconstruction
97.02 to 82.46 ms (15.0%). A separate 41-pair confirmation reduced 94.67 to
81.16 ms (14.3%); paired median change was -13.4%, with a bootstrap interval of
-15.0% to -12.6%. Every output matched independent source SHA-256 expectations.

Separate 21-pair native memory checks reduced median peak commit from 23.00 to
12.74 MiB (44.6%). Peak working set stayed near 73.4 MiB; no working-set reduction
is claimed. All final post-exit high-water reads succeeded with nonzero data.

A renamed-file control still built the source chunk index and reused chunks after
three whole-file copies. Its timing results varied: a 41-pair analysis suggested
a small slowdown, but an isolated 81-pair confirmation did not establish a change.
The no-source reconstruction control was also inconclusive. No general update or
partial-reuse speedup is claimed. The strengthened planner regression confirms
that a changed file can reuse chunks from a file already copied whole.
Evidence: `.artifacts/tests/research_optimization_pass/round34_report.md`.


## Limit source chunk locations to partial targets, October 2026

When target files that need chunk work have fewer chunk references than the
verified source, planning retains only source locations for those target hashes.
Otherwise it keeps the direct full-source index path without allocating a hash
filter. This is a reference-count bound, not a distinct-chunk count. Whole-file
copy eligibility has one shared predicate for job selection and filter selection;
a named preparation helper keeps this work outside the main file/job loop.

Every verified source file is searched, including files already selected for
whole copying. Excluded chunks still advance the physical source offset. Source
eligibility, verification and reused-byte hashing remain unchanged.

On Windows, a four-file 64 MiB / 53,822-chunk fixture renamed one file: three files
copied whole, while the renamed file reused source chunks. An initial guarded
41-pair comparison reduced median reconstruction 193.93 to 186.88 ms (3.6%).
The final build, after separating the preparation helper, reduced 211.18 to
198.79 ms over 41 pairs (5.9%); paired median change was -6.4%, with a bootstrap
interval of -8.4% to -4.8%. Every output matched independent source SHA-256 maps.

Separate 21-pair native checks reduced median peak commit from 32.55 to 23.30 MiB
(28.4%). Peak working set stayed near 75 MiB; no working-set reduction is claimed.
All 84 final post-exit high-water queries across partial/full-source controls
succeeded with nonzero data. Full-source control peak commit remained near flat.

All-files-renamed, whole-copy and no-source timing controls did not establish
consistent changes. These results concern partial reuse with a smaller target
working set, not general update throughput. The strengthened regression selects
a middle chunk after an excluded prefix from a source file already copied whole,
and checks the exact source path and byte offset. Evidence:
`.artifacts/tests/research_optimization_pass/round35_report.md`.


## Reuse source chunk identity and path storage, October 2026

The source catalog selects one location for each chunk hash. Planning now keys
its source-job lookup by that hash, after validating raw size against the chosen
location, instead of cloning a path/offset/size/hash tuple for every reference.
Locations and jobs share one immutable native path allocation per source file.
Source-job ordering still compares path values and offsets; source eligibility,
byte hashing, file-handle ownership and publication ordering remain unchanged.

On Windows with four workers, a four-file 64 MiB / 53,822-chunk fixture renamed
all files and reused every chunk from the source. An initial 41-pair comparison
reduced median reconstruction 520.17 to 460.72 ms (11.4%). A separate 41-pair
confirmation reduced 520.14 to 472.11 ms (9.2%); paired median change was -10.2%,
with a bootstrap interval of -11.5% to -8.5%. A 21-pair ablation showed path
sharing was another 2.9% faster than compact lookup keys alone.

With only one renamed file, 41 pairs reduced 183.53 to 176.32 ms (3.9%). Every
restored tree matched independently calculated source SHA-256 expectations.
The whole-copy control was inconclusive. The no-source control was slightly
favorable in one comparison; no general reconstruction gain is claimed.

Separate 21-pair native checks reduced median peak commit from 61.59 to 34.18 MiB
(44.5%) for full-source reuse and from 23.31 to 16.64 MiB (28.6%) for partial reuse.
Peak working sets stayed near 74 MiB; no working-set reduction is claimed. All
84 final post-exit queries succeeded. Savings depend on chunk count and path
length; these are fixture-specific results, not a global memory bound.

The expanded regression checks repeated source hashes within/across files,
first-location selection among duplicate source candidates, distinct source
hashes sharing path storage, exact offsets and every target range. A wrong raw
size on an already-seen hash must fail instead of reusing its cached job.
Evidence: `.artifacts/tests/research_optimization_pass/round36_report.md`.


## Inline first source destination, October 2026

Source-chunk jobs now store their mandatory first destination directly, with a
vector only for additional destinations. This avoids a target allocation for
single-destination jobs without adding a dependency or changing artifact formats.
Repeated destinations preserve insertion order through one shared iterator used
by writing, scheduling and preview byte accounting.

Separate 21-pair Windows native checks on the four-file 64 MiB / 53,822-chunk
fixture reduced median peak committed memory from 35,782,656 to 31,731,712 bytes
(11.3%) when all files were renamed and every chunk reused from source. With
only one renamed file, peak commit fell from 17,416,192 to 16,715,776 bytes (4.0%).
Peak working sets remained near 74 MiB; no working-set reduction is claimed.
All 84 final post-exit high-water queries succeeded and restored trees matched
independent SHA-256 expectations.

Timing evidence was mixed. The seven-pair full-source screen was 3.0% faster;
a 41-pair comparison was 0.8% slower (paired interval +0.3% to +1.3%). A separate
41-pair confirmation had a paired median change of +0.1%, with an interval of
-1.0% to +1.4%. Its ratio of separate medians was 1.1% faster. No runtime gain or
consistent runtime regression is established across runs. Partial-reuse and
no-source paired intervals crossed zero; whole-copy was slightly favorable in
the larger run. Retention is based on the lower measured peak commit, not speed.
These results concern this host and fixture, not general throughput or a global
memory bound. Evidence: `.artifacts/tests/research_optimization_pass/round37_report.md`.


## Bounded source-worker batches, October 2026

Source-chunk jobs retain their individual hashing, writes and producer/file
relationships, but adjacent ready jobs can share one sequential worker task.
Admission limits groups to 32 jobs and 64 KiB of source bytes, within one source
path and the first job's memory reservation. Larger chunks run independently.
One terminal event carries the original job indexes and timings; the coordinator
counts all their producers before queuing file verification. Handles and permits
are released before that event, and failure drains dispatched tasks.

On Windows with four workers, 21 alternating pairs over four 16 MiB files with
53,822 source-chunk references reduced median full-source reconstruction from
456.12 to 340.82 ms (25.3%). Paired median change was -26.6%, with a bootstrap
interval of -29.1% to -23.6%. With only one renamed file, medians fell from
196.81 to 151.23 ms (23.2%); paired median change was -18.2%, interval -24.0%
to -16.1%. Every restored tree matched independent source SHA-256 expectations.

The initial 1 MiB batch limit also improved small-chunk reuse, but slowed a
larger-chunk control in repeated runs; it was rejected. The final 64 KiB limit
was inconclusive over 41 pairs with 421 larger chunk references. Whole-copy,
source-fanout, artifact-only and artifact-fanout paired intervals also crossed
zero. No general reconstruction or fanout speedup is claimed.

Separate 21-pair native checks showed peak commit near flat: partial reuse
16,867,328 to 16,891,904 bytes, full source reuse 31,756,288 to 31,715,328 bytes.
All 84 final post-exit high-water queries succeeded; no memory reduction is
claimed. Worker tests cover count/byte/reservation limits, source paths, work
boundaries and unqueued index gaps, success counts/timings, hash mismatch,
cancellation, stopping later writes and resource release before the one terminal
event. Evidence: `.artifacts/tests/research_optimization_pass/round40_report.md`.


## Combined reads inside source-worker batches, October 2026

An admitted source group with contiguous offsets now uses one exact-range read
and one shared byte buffer, bounded by the existing 64 KiB group limit. Every
chunk is still hashed against its own expected identity before its targets are
written. Gapped groups keep separate chunk reads; there is no read-ahead across
excluded ranges or cache of chunk bytes between worker tasks. The existing
1 MiB source-I/O reservation covers this buffer. Single jobs keep their read path.
Source I/O owns both paths and the shared hash/write implementation; workers
retain admission, dispatch, resource release and the single terminal event.

On Windows with four workers and four 16 MiB files / 53,822 source references,
a 41-pair prototype comparison reduced full-source reconstruction from 345.76 to
311.24 ms (10.0%). After removing an unused file-plan argument, the final build
reduced 337.13 to 303.05 ms over 21 pairs (10.1%); paired median change was -8.4%,
with an interval of -10.2% to -6.2%. Partial reuse reduced 147.33 to 138.95 ms
(5.7%); paired median change was -5.2%, interval -6.5% to -4.4%. Every restored
tree matched independent SHA-256 expectations. These gains are additional to
bounded worker batching, which is present in the baseline.

Whole-copy, larger-chunk, source-fanout, artifact-only and artifact-fanout paired
intervals crossed zero. Separate native checks showed peak commit near flat;
all 84 final post-exit queries succeeded. No general throughput or memory gain
is claimed. A positioned-read alternative was screened and discarded first.

A combined read failure writes none of that group's ranges. Later hash failure
or cancellation stops before later writes; worker tests also cover the gapped
fallback and one-event resource-release ordering. Read time is charged once to
the first chunk record, preserving aggregate timings and per-chunk byte counts.
Combined reads have an `installation.source_batch.read` trace span; chunk byte
hash/write timings remain per job. Evidence:
`.artifacts/tests/research_optimization_pass/round42_report.md`.


## Shared bounded output for source batches, October 2026

Contiguous source batches now reuse the existing artifact output-range writer.
It combines adjacent verified single-destination writes in one 64 KiB buffer;
fanout writes bypass byte buffering. The same worker-local handle policy serves
both paths. The type is named `ReconstructionOutputWriter` to reflect these two
users. Single source jobs and gapped groups retain direct writes. Combined input
and output buffers fit within the existing source-I/O reservation.

On Windows with four workers and the four-file 64 MiB / 53,822-reference fixture,
41 alternating pairs reduced full-source reconstruction from 301.32 to 164.10 ms
(45.5%). Paired median change was -45.1%, with an interval of -45.3% to -44.6%.
Partial reuse reduced 141.24 to 106.96 ms (24.3%); paired median change was -24.4%,
interval -25.3% to -23.8%. Baseline already includes bounded source-worker tasks
and combined input reads. Every restored tree matched independent SHA-256 maps.

Whole-copy, larger-chunk, source-fanout, artifact-only and artifact-fanout paired
intervals crossed zero. Separate 21-pair native checks showed peak commit near
flat, with all 84 final post-exit queries successful. No general reconstruction
or memory gain is claimed.

The source batch flushes before reporting success, charging final flush duration
once to its last chunk record. Cancellation or a later hash mismatch may discard
earlier verified bytes still pending in the private buffer. Already flushed
verified ranges can remain in staging, but failure cannot publish the operation.
The expanded worker test checks cross-file fanout, final-flush failure, no retry
on drop, pending-byte discard, byte totals and resource release before one
terminal event. Existing shared-writer tests cover gaps, target switches, large
ranges and flush failures. Evidence:
`.artifacts/tests/research_optimization_pass/round43_report.md`.


## Dense scheduled-job flags, October 2026

Reconstruction scheduling now uses one boolean flag per immutable planned job
index instead of a hash set. Initial queue construction sets ready flags, while
downloaded readiness checks and sets the same flag before enqueueing. The existing
enqueue count supplies cardinality. Flags are allocated at first use inside the
initial-dispatch profile span, so measured dispatch work includes that allocation.

A detailed-profile comparison on Windows with four workers and 53,822 source
jobs reduced median initial dispatch from 3.15 to 1.76 ms over 15 alternating
pairs (44.0%). Every restored tree matched independent SHA-256 expectations and
no profile events were dropped. Earlier loop-only measurements also improved,
but excluded the new flag allocation; the final figure includes it.

Whole-operation trials were mixed: favorable partial-reuse measurements did not
establish a consistent general gain, and the final 21-pair whole-copy, partial,
full-source, larger-chunk, source-fanout and artifact-only/fanout intervals all
crossed zero. Detailed profiling adds per-job queue timestamps, so this phase
result must remain separate from unprofiled reconstruction throughput.

Separate native trials showed peak commit near flat, with all 84 final post-exit
queries successful. Retention is based on lower measured initial-dispatch work
and direct membership storage for dense job indexes, not a claimed whole-process
memory saving or general reconstruction speedup. Evidence:
`.artifacts/tests/research_optimization_pass/round45_report.md`.


## CLI builds in GitHub Actions

`.github/workflows/build-cli.yml` builds `gutch-cli` in release mode with the
locked Cargo dependencies and Rust 1.97.1. It runs manually or when relevant
code/build files change on `main`. Each native runner executes `--help` before
packaging its executable. The independent correctness workflow remains the
owner of the full test suites.

| Artifact | Rust target | Native runner |
|---|---|---|
| `gutch-cli-linux-x86_64` | `x86_64-unknown-linux-gnu` | Ubuntu 24.04 x86-64 |
| `gutch-cli-linux-arm64` | `aarch64-unknown-linux-gnu` | Ubuntu 24.04 ARM64 |
| `gutch-cli-windows-x86_64` | `x86_64-pc-windows-msvc` | Windows Server 2025 x86-64 |
| `gutch-cli-windows-arm64` | `aarch64-pc-windows-msvc` | Windows 11 ARM64 |

Download artifacts from a successful **Build Gutch CLI** run. Each package
contains the executable, `SHA256SUMS`, commit/compiler/target provenance in
`build.json`, source notices, and target-specific Cargo dependency license files
indexed by `dependencies.json`, plus Rust standard-library notices. Missing
notices fail packaging.
These are unsigned CI builds retained for 30 days. Linux builds use GNU libc
from Ubuntu 24.04; runtime system libraries are not bundled.

`_tools/package_cli.py` stages packages under `.builds/releases/<target>/`.
Intermediate compilation and dependency caches stay under
`.builds/compiler-cache/cargo/`; neither binaries nor generated notices enter Git.
