# Gutch Reconstruction Pipeline

Status: Current

Scope: Gutch component current state

## Boundary

The reconstruction pipeline turns a snapshot blueprint and repo chunk index
into a verified filesystem tree. It can source artifacts from the local repo,
reuse a verified source snapshot, or consume bundles downloaded by the remote
fetch path. Required content is written through local staging. Reconstruction publishes the
completed tree through a locked, recoverable backup-and-rename sequence. Repair
uses the same executor with selective entry publication; its failure boundary is
described in [installation policies](03_verification_and_repair.md).

In the current remote flow, “remote reconstruction” means remote artifacts are
made available to the local engine; Gutch does not reconstruct into a remote
filesystem target. The destination, staging directory, output handles, writes,
and final verification are local operations.

Installation orchestration owns staging and publication. Pipeline coordination
owns artifact readiness, file dependency completion and verification scheduling.
Workers own memory admission and execution. Completion releases worker handles
before notification; failure drains dispatched work before staging cleanup.

## Execution timeline

| Stage | Work | Overlap and gate |
| --- | --- | --- |
| 1. Load inputs | Read repo metadata, snapshot metadata, blueprint, chunk index, and compression profile. | No reconstruction job starts before the plan has valid inputs. |
| 2. Locate reusable data | Mark required local artifact files and, when supplied, verify the source snapshot directory. | Existing artifact presence is used for planning; artifact contents are checked when read. |
| 3. Build the job graph | Convert file chunk sequences into deduplicated artifact, source-chunk, or whole-file-copy jobs. | One artifact job can fan out to many files and offsets. |
| 4. Prepare staging | Validate blueprint paths, create the staging root and directories, resolve file paths, and size output files. | Path resolution and file-path preparation can use reconstruction workers. |
| 5. Dispatch ready work | Run local artifact jobs, source-chunk jobs, and source-file copies. | Independent jobs start immediately; files wait only on their own dependency count. |
| 6. Fetch missing remote artifacts | Download missing bundles through the transfer scheduler and emit `Ready` only after an atomic local write completes. | In unified fetch-reconstruct, download and decode overlap. |
| 7. Decode and fan out | Read each artifact, verify selected raw chunks, and positional-write bytes to all target files. | Artifact jobs run concurrently; a bundle is read once per artifact job. |
| 8. Verify completed files | Enqueue each file check when all producing jobs for that file finish; admit adjacent ready checks in bounded batches. | Unrelated files continue while another file is waiting on an artifact. |
| 9. Finalize and promote | Apply file, directory, and symlink metadata, then publish through the selected directory or selective-entry policy. | Promotion is gated on successful reconstruction and verification; directory replacement removes the old tree; selective repair handles extras separately. |

## Job graph and parallelization

The planner is file-aware and artifact-aware at the same time. It keeps a
file-major blueprint view for output and verification, then creates an
artifact-major execution view:

- A complete matching source file becomes one `CopySourceFile` job.
- A matching source chunk becomes one `SourceChunk` job, even when several
  target files use the same source offset and hash.
- All remaining references to one `ArtifactRef` become one artifact job.
- Every selected chunk records all target file indexes and offsets, so one
  decoded chunk can be written to several files.

The artifact-job map owns the distinct artifact identities for the plan. The
required-artifact set and bundle count are derived once from that map, including
cached artifacts and excluding source-only jobs. Bundle references retain the
repository index's shared ID storage; provider IDs and reported IDs remain text.
Each distinct artifact-backed chunk hash resolves its indexed location and job
once during planning. Repeated references reuse that job and chunk position;
every reference still checks its declared size against the indexed raw size.
The lookup is local to one plan and is not used after jobs or chunks are sorted.
Missing-file dependencies are derived from the flattened artifact-job targets.
Availability is checked once per artifact job, and each dependent file is counted
once for that artifact. Copied files and source-chunk jobs add no artifact dependency.

During planning, temporary target lists use the existing job and chunk insertion
positions as vector indexes. Copy/source jobs keep empty slots in that layout.
Target ranges are flattened before jobs or chunks are sorted, preserving every
file index and offset. Content identities still use their existing lookup maps.

The reconstruction pool has the runtime-planned worker count. The dispatcher
starts all jobs that are ready from local artifacts or verified source data.
Scheduled membership is one boolean flag per planned job index. Initial ready
jobs set their flags during queue construction; downloaded readiness only queues
an unset job, preventing duplicate dispatch. The existing enqueue count tracks
how many distinct jobs were scheduled.
Each job completion decrements the dependency count for its target files. A
file is eligible for its final check only when its count reaches zero; the
scheduler gives these newly completed files priority over queued producing
jobs. Empty-file checks need no such priority.
Verification never starts while a producer for that file is still running.

Source-chunk admission groups adjacent ready jobs from the same source path into
one sequential worker task, up to 32 jobs and 64 KiB of source bytes. A larger
chunk runs alone. A group stops at another work kind, another source path, a
nonadjacent job index, or a chunk needing a larger memory reservation than its
first job. The existing source-I/O reservation covers the group. When its source
offsets are contiguous, one exact-range read fills a buffer of at most 64 KiB;
each chunk hashes its own slice before writing its targets. Groups containing
gaps retain individual chunk reads, without reading excluded source ranges.
The shared buffer lives only for that worker task. Each chunk retains its own
cancellation check before hashing/writing. Success sends one terminal event containing
all completed job indexes and per-job timings. The coordinator advances progress
by the job count and decrements every original producer/file dependency. A failed
or cancelled group stops immediately; it cannot make its files verification-ready.
Worker handles, context and the permit are released before the terminal event.
Draining counts dispatched tasks, including groups, rather than constituent jobs.

Contiguous source groups also use the artifact path's shared 64 KiB output-range
writer. Adjacent single-destination ranges can be combined; file/offset changes
flush the pending range, and multi-target chunks bypass byte buffering. Its local
handle references live for the admitted worker task. The input and output buffers
fit inside the existing 1 MiB source-I/O reservation. Single jobs and gapped source
groups keep direct writes. Final flush must succeed before the completion event;
its duration is charged once to the last chunk record. Failure or cancellation
discards pending bytes without a destructor flush. Previously flushed verified
ranges may remain in staging; an error prevents publication of failed work.

Verification admission groups adjacent ready checks into one worker task, up to
32 files and 1 MiB of expected file bytes. A file larger than that byte limit
runs alone. Groups stop at queued producing jobs; batching does not change
artifact readiness or move checks across those jobs. Every file retains its
own cancellation checkpoint and fresh output-size query. One terminal event reports the batch's verified file count and
summed bytes/timings; progress advances by that count. Failure or cancellation
stops the batch, and worker context and memory permits are released before its
terminal event. The coordinator drains dispatched batches before cleanup.

The coordinator tracks ready work separately from dispatched work. Memory
admission can skip a temporarily blocked producer and run a ready verification.
On failure, queued work is discarded and dispatched workers are drained before
their output resources are released. Download readiness remains separate from
this reconstruction queue.

Remote transfer has a separate worker limit. The default transfer policy uses
four workers, while adaptive transfer can change the active count up to sixteen
based on throughput, retries, and downstream backpressure. Reconstruction
workers and transfer workers therefore control different resources. The
reconstruction side reports backpressure when its outstanding job queue grows
beyond roughly two jobs per reconstruction worker.

## Artifact read behavior

Bundled storage opens one bundle reader for the artifact job, identifies the
required bundle entries, and streams decoded chunks in bundle-table order. It
verifies each selected raw chunk with BLAKE3, validates declared raw sizes and
full encoded bundle identity, and writes verified bytes to every recorded target. Unneeded bundle entries
may be decoded while advancing the stream but are not retained as output.

Loose-chunk storage reads one compressed artifact for its one requested chunk,
validates the complete encoded size recorded on the artifact job and the decoded
raw hash, and writes that chunk to all of its target offsets. Both storage modes use the same job graph, staging rules,
chunk verification, file dependency counters, and final file verification.

Each artifact worker coalesces consecutive single-target ranges in one lazy
64 KiB output buffer. File switches, offset gaps/backward movement and capacity
limits flush pending bytes. Ranges at least 64 KiB and chunks with multiple
targets write directly after flushing any pending range, avoiding repeated
fanout copies. Every chunk is verified before buffering. An explicit final
flush completes before job success, file readiness or whole-file hash forwarding;
flush failures propagate. Cancellation/error drops private pending bytes without
writing during destruction. Positional writes and worker-local handles remain
with the handle/I/O owner, independently of decoding and scheduling.

Artifact work uses the automatic/explicit working-memory budget policy described
for [capture](../03_capture/02_scan_pipeline.md#parallelization-and-resource-limits). The
coordinator admits jobs only when both a worker slot and their memory reservation
are available. Waiting jobs stay queued; eligible verification and smaller jobs
can proceed. Artifact jobs reserve 136 MiB for the bounded decoder window and
chunk buffer, plus 64 KiB for coalesced output. A budget exactly equal to the old
136 MiB reservation therefore needs another 64 KiB. Source-copy work
reserves only its I/O buffer. This limits admitted artifact work, not total
process RSS, which also includes indexes, worker stacks, and other allocations.

## Reuse and locality

The planner prefers reuse in this order:

1. Copy an entire source file when path, size, and full-file hash match.
2. Read matching chunks directly from a verified source snapshot when only
   parts of a file can be reused.
3. Read the content-addressed bundle or loose chunk from the local repo.
4. Wait for the remote scheduler to download a missing artifact, then use the
   same local artifact job.

A damaged reuse source contributes only files that passed verification. Healthy
files remain eligible for whole-file or chunk reuse; chunks inside damaged files
are not salvaged. Reused bytes are validated again while staging.
The planner builds source chunk locations only when a target file cannot be copied
whole. It searches every verified source file, including files already selected
for whole-file copying. If the non-copy targets contain fewer chunk references
than the source, the index retains only hashes those targets require; otherwise
it retains the full source index. Source offsets include excluded chunks.
Source verification still runs before this derived index is built.

Source-chunk jobs are deduplicated by chunk hash after checking the requested
raw size against the chosen source location. The source catalog fixes one path
and offset per hash. Locations and jobs share one immutable path per source file.
Every source job stores its first destination inline; only additional destinations
need a target-vector allocation. Writing, scheduling and preview accounting visit
the first destination followed by those additional destinations.
Source handles are cached (up to 64 entries), and source jobs are ordered by
path and offset to reduce reopen/seek work. Artifact job targets occupy flat
arrays and refer to typed chunk hashes. Artifact jobs are deduplicated by artifact identity. This prevents repeated
decoding while preserving positional writes for files whose chunks are shared.

Output file handles are shared through a bounded LRU cache with a maximum of
256 open handles. Writes use positional operations, so concurrent artifact
jobs can fill different ranges without serializing an entire file through one
worker. The cache stores handles, not decoded chunk bytes; a decoded chunk is
verified and fanned out during its artifact job.

On Windows, files of at least 64 MiB with multiple producing jobs use independent
file objects in each worker task's local handle cache when multiple workers are enabled.
File sizes and producer counts come from the existing reconstruction plan; no
extra scan or runtime benchmark selects this path. Creation and sizing remain
centralized, and independent opens never truncate existing ranges. Other files
and platforms retain shared handles. Plans with at most 64 output files and no
independent handles retain up to 64 local references to the same file objects
already held by the shared cache. Larger plans and plans using any independent
handles retain the eight-entry local limit. The expanded shared-reference cache
does not open additional file objects.

## Local, cached, and remote targets

### Local or cached repository

`reconstruct_snapshot` uses the local snapshot manifest, chunk index,
and artifacts. Missing local artifacts fail unless a source snapshot supplies
the required content. There is no download scheduler or HTTP client in this
path. A local artifact is initially considered available when its expected path
is a file; decoding and hash checks establish whether it is actually usable.

### Remote fetch followed by reconstruction

Standalone fetch resolves metadata and missing artifact URLs, downloads all
selected artifacts, waits for those downloads to finish, and then reconstruction
uses the resulting local repo files. This has a simpler phase boundary but no
download/decode overlap.

### Unified remote fetch-reconstruct

`fetch_and_reconstruct_snapshot` prepares the local metadata and
reconstruction plan, resolves direct bundle URLs or asks the injected
`ArtifactUrlProvider` for unresolved artifact URLs, and starts a scheduler for the
missing artifacts. The scheduler writes each bundle atomically and sends a
ready event only after the final local path is complete. The reconstruction
dispatcher consumes ready events while local and source jobs continue, allowing
download, decode, hash, and output writes to overlap.

Artifact URLs must be absolute `http` or `https` URLs. Each artifact body is
limited to 256 MiB, and a body that stalls for 60 seconds fails. Download
errors never include the URL, which may carry an access token. Connection,
body and retryable status failures (408, 429, 5xx) are retried up to three
times for artifacts and snapshot metadata alike; malformed requests, redirect
policy failures and oversized bodies are not. A failed attempt's bytes are
withdrawn from the transfer totals, so retries do not inflate them. When one
worker fails, its error is reported before sibling workers are cancelled.

The provider supplies artifact locations; it does not move authority into
Gutch. Backend authorization, session context, and entitlement decisions stay
with the caller and its provider adapter. Remote fetch also bounds metadata,
blueprint, and chunk-index sizes before they enter the local planning path.

Once every download worker reports completion, reconstruction stops polling
the download event channel and finishes outstanding reconstruction and
verification work. Channel closure before all workers finish remains an error.

## Staging, verification, and promotion

Staging is created beside the destination unless the caller supplies another
valid staging directory. It is removed on failure. The destination is not
replaced until all producing jobs, per-file verification, metadata finalization,
and promotion checks succeed. This protects the previously committed tree from
partial reconstruction. A successful replacement promotes the complete target
tree, so source-only files and unrelated destination entries do not carry over
to the new snapshot.

Reconstruction does not reread its output to hash it. Every byte it writes was
already checked before the write: artifact and source-chunk jobs compare each
raw chunk's BLAKE3 before writing it to any target, and whole-file copies hash
the full file while copying and compare it with the expected full-file hash.
Planning proves that each file's chunk sizes add up to exactly its expected size,
so verified chunks at their planned offsets reproduce the expected file. After
all producing jobs for a file finish, its final check confirms only that the
completed output has the expected size. Damage after a successful write, such
as a storage fault, is outside this check; explicit verification
([`02_verify_pipeline.md`](02_verify_pipeline.md)) reads and hashes the
installed files.

## Timeline and diagnostic evidence

The operation spans `snapshot.reconstruct`, `snapshot.reconstruct.apply`, and
`snapshot.reconstruct.pipeline`. Traces expose preparation, dispatch, bundle
or source jobs, chunk verification, positional writes, file verification,
finalization, and promotion. Reconstruction timing separates prepare, artifact
read, bundle read/decompression, chunk verification, rebuild/write, file
verification, finalization, promotion, and total wall phases. Remote results
also include transfer bytes, rate, retries, peak concurrency, and concurrency
adjustments.

## Failure boundaries

- Invalid blueprint paths, missing required artifact coverage, corrupt framing,
  decompression failure, size mismatch, chunk hash mismatch, source mismatch,
  or file hash mismatch fails the operation.
- A failed download never becomes a ready artifact; a failed scheduler event
  fails the reconstruction that depends on it.
- Cancellation is checked at job and write boundaries and cancels pending
  transfer work.
- Failed staging is removed, while the prior destination remains authoritative
  because promotion has not occurred.
- Reusing a local path is not itself proof of integrity; the reader and file
  verifier provide the integrity gates.

Workers release their shared context before reporting completion. Once the
coordinator has received every completion and released its own context, cached
output handles cannot remain owned by an exiting worker during publication.

## Implementation anchors

The owning implementation is `src/09_installation/plan.rs` for planning,
`src/09_installation/execute/` for staging, dispatch, and promotion
(`job_io.rs` for artifact/source reads), `src/09_installation/install.rs` for
local reconstruction entry, `src/09_installation/fetch.rs` for combined remote fetch/reconstruction,
`src/08_sync/fetch.rs` for repository acquisition and publication, and
`src/05_transfer/download.rs` for transfer scheduling. Adaptive decisions and
feedback have separate owners under `transfer`; bounded output/source handles
and positioned writes belong to `09_installation/execute/handles.rs`.

## Related pages

- [`04_packaging_and_bundles.md`](../03_capture/04_packaging_and_bundles.md) — blueprint,
  chunk-map, bundle, and loose-chunk contracts.
- [`02_verify_pipeline.md`](02_verify_pipeline.md) — file verification and
  classification after reconstruction.
- [`03_verification_and_repair.md`](03_verification_and_repair.md)
  — install lifecycle and repair behavior around the engine.
- [`04_path_safety_and_conflicts.md`](04_path_safety_and_conflicts.md) — path
  containment, staging, and overwrite rules.
- [`01_performance_and_benchmarks.md`](../06_development/01_performance_and_benchmarks.md) —
  runtime, transfer, tracing, and throughput interpretation.
