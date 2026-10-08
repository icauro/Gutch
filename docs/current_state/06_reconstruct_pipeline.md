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
described in [installation policies](08_installation_verification_and_repair.md).

In the current remote flow, “remote reconstruction” means remote artifacts are
made available to the local engine; Gutch does not reconstruct into a remote
filesystem target. The destination, staging directory, output handles, writes,
and final verification are local operations.

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
| 8. Verify completed files | Enqueue a file verification job when all producing jobs for that file finish. | Unrelated files continue while another file is waiting on an artifact. |
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

The reconstruction pool has the runtime-planned worker count. The dispatcher
starts all jobs that are ready from local artifacts or verified source data.
Each job completion decrements the dependency count for its target files. A
file is eligible for full-file verification only when its count reaches zero;
the scheduler gives these newly completed files priority over queued producing
jobs. This overlaps hashing with remaining reconstruction and can read recently
written pages from the filesystem cache. Empty-file checks need no such priority.
Verification never starts while a producer for that file is still running.

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
validates the stored size and decoded raw hash, and writes that chunk to all of
its target offsets. Both storage modes use the same job graph, staging rules,
chunk verification, file dependency counters, and final file verification.

Artifact work uses the automatic/explicit working-memory budget policy described
for [capture](04_scan_pipeline.md#parallelization-and-resource-limits). The
coordinator admits jobs only when both a worker slot and their memory reservation
are available. Waiting jobs stay queued; eligible verification and smaller jobs
can proceed. Artifact jobs reserve 136 MiB for the bounded decoder window and
chunk buffer. Source-copy work
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

Source-chunk jobs are deduplicated by source path, offset, raw size, and hash.
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
file objects in each job's local handle cache when multiple workers are enabled.
File sizes and producer counts come from the existing reconstruction plan; no
extra scan or runtime benchmark selects this path. Creation and sizing remain
centralized, and independent opens never truncate existing ranges. Other files
and platforms retain shared handles. Each active job keeps at most eight local
handles, in addition to the bounded shared cache.

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

For ordinary artifact and source-chunk jobs, the per-file verifier reads the
completed output and checks size and full-file BLAKE3. A whole-file copy hashes
while copying, so its completed hash can be forwarded to verification without a
second full read. Large-file hash mode considers both the reconstruction worker
count and the number of output files, so one large output can use the existing
pool for parallel hashing. The exact threshold and modes are described in
[`07_verify_pipeline.md`](07_verify_pipeline.md).

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

The owning implementation is `src/installation/execute.rs` for planning,
staging, dispatch, and promotion, `src/installation/io.rs` for
artifact/source reads, `src/installation/apply.rs` for
local reconstruction entry, `src/installation/fetch.rs` for combined remote fetch/reconstruction,
`src/repo/fetch.rs` for repository acquisition and publication, and
`src/transfer/download.rs` for transfer scheduling. Adaptive decisions and
feedback have separate owners under `transfer`; bounded output/source handles
and positioned writes belong to `installation/handles.rs`.

## Related pages

- [`05_packaging_and_bundles.md`](05_packaging_and_bundles.md) — blueprint,
  chunk-map, bundle, and loose-chunk contracts.
- [`07_verify_pipeline.md`](07_verify_pipeline.md) — file verification and
  classification after reconstruction.
- [`08_installation_verification_and_repair.md`](08_installation_verification_and_repair.md)
  — install lifecycle and repair behavior around the engine.
- [`11_path_safety_and_conflicts.md`](11_path_safety_and_conflicts.md) — path
  containment, staging, and overwrite rules.
- [`12_performance_and_development.md`](12_performance_and_development.md) —
  runtime, transfer, tracing, and throughput interpretation.
