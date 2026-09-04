# Gutch Reconstruction Pipeline

Status: Current

Scope: Gutch component current state

## Boundary

The reconstruction pipeline turns a snapshot blueprint and repo chunk index
into a verified filesystem tree. It can source artifacts from the local repo,
reuse a verified source snapshot, or consume bundles downloaded by the remote
fetch path. It always writes the destination through a local staging directory
and promotes the completed tree atomically.

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
| 9. Finalize and promote | Apply file, directory, and symlink metadata, then atomically replace the destination with staging. | Promotion is gated on successful reconstruction and verification. |

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
the scheduler does not wait for unrelated files.

Remote transfer has a separate worker limit. The default transfer policy uses
four workers, while adaptive transfer can change the active count up to sixteen
based on throughput, retries, and downstream backpressure. Reconstruction
workers and transfer workers therefore control different resources. The
reconstruction side reports backpressure when its outstanding job queue grows
beyond roughly two jobs per reconstruction worker.

## Artifact read behavior

Bundled storage opens one bundle reader for the artifact job, identifies the
required bundle entries, and streams decoded chunks in bundle-table order. It
verifies each selected raw chunk with BLAKE3, checks raw and compressed sizes,
and writes the verified bytes to every recorded target. Unneeded bundle entries
may be decoded while advancing the stream but are not retained as output.

Loose-chunk storage reads one compressed artifact for its one requested chunk,
validates the stored size and decoded raw hash, and writes that chunk to all of
its target offsets. Both storage modes use the same job graph, staging rules,
chunk verification, file dependency counters, and final file verification.

## Reuse and locality

The planner prefers reuse in this order:

1. Copy an entire source file when path, size, and full-file hash match.
2. Read matching chunks directly from a verified source snapshot when only
   parts of a file can be reused.
3. Read the content-addressed bundle or loose chunk from the local repo.
4. Wait for the remote scheduler to download a missing artifact, then use the
   same local artifact job.

Source-chunk jobs are deduplicated by source path, offset, raw size, and hash.
Artifact jobs are deduplicated by artifact identity. This prevents repeated
decoding while preserving positional writes for files whose chunks are shared.

Output file handles are shared through a bounded LRU cache with a maximum of
256 open handles. Writes use positional operations, so concurrent artifact
jobs can fill different ranges without serializing an entire file through one
worker. The cache stores handles, not decoded chunk bytes; a decoded chunk is
verified and fanned out during its artifact job.

## Local, cached, and remote targets

### Local or cached repository

`reconstruct_snapshot_from_repo` uses the local snapshot manifest, chunk index,
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

`download_and_reconstruct_snapshot` prepares the local metadata and
reconstruction plan, resolves direct bundle URLs or asks the injected
`BundleUrlProvider` for unresolved bundle URLs, and starts a scheduler for the
missing artifacts. The scheduler writes each bundle atomically and sends a
ready event only after the final local path is complete. The reconstruction
dispatcher consumes ready events while local and source jobs continue, allowing
download, decode, hash, and output writes to overlap.

The provider supplies artifact locations; it does not move authority into
Gutch. Backend authorization, session context, and entitlement decisions stay
with the caller and its provider adapter. Remote fetch also bounds metadata,
blueprint, and chunk-index sizes before they enter the local planning path.

## Staging, verification, and promotion

Staging is created beside the destination unless the caller supplies another
valid staging directory. It is removed on failure. The destination is not
replaced until all producing jobs, per-file verification, metadata finalization,
and promotion checks succeed. This protects the previously committed tree from
partial reconstruction.

For ordinary artifact and source-chunk jobs, the per-file verifier reads the
completed output and checks size and full-file BLAKE3. A whole-file copy hashes
while copying, so its completed hash can be forwarded to verification without a
second full read. Large-file hash mode uses the reconstruction runtime policy;
the exact threshold and modes are described in
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

## Implementation anchors

The owning implementation is `src/snapshot_ops/pipeline.rs` for planning,
staging, dispatch, and promotion, `src/snapshot_ops/reconstruct_io.rs` for
artifact/source reads and positional writes, `src/snapshot_ops/apply.rs` for
local reconstruction entry, `src/snapshot_ops/fetch.rs` for unified remote
fetch-reconstruct, and `src/snapshot_ops/transfer.rs` for transfer scheduling.

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
