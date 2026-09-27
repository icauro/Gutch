# Gutch Scan and Capture Pipeline

Status: Current

Scope: Gutch component current state

## Boundary

The scan pipeline turns one selected source directory into a snapshot file
manifest, chunk references, and new repo artifacts. It owns filesystem
enumeration, file and chunk hashing, previous-snapshot reuse, global chunk
deduplication, bundle or loose-chunk packaging, and snapshot-manifest
finalization. Source selection and tag identity are defined in
[`03_snapshot_source_ingestion.md`](03_snapshot_source_ingestion.md).

The pipeline reads source content and writes repo artifacts. It does not decide
creator access, authorize remote storage, or commit app install metadata.

## Execution timeline

The capture operation runs these stages in order. Parallel stages use the same
capture Rayon pool unless noted otherwise.

| Stage | Work | Parallel behavior |
| --- | --- | --- |
| 1. Source and identity | Resolve selected source roots, normalized paths, tags, and snapshot identity. | Performed before capture work begins. |
| 2. Runtime setup | Resolve `jobs` into a bounded worker count and create the capture worker pool. | One operation-local pool is shared by scan and packaging. |
| 3. Inventory | Walk each source with `WalkDir`, without following links, in filename order. Record regular files, directories, and supported symlinks. | Directory enumeration is ordered; metadata conversion can run on the pool. |
| 4. File scan | Read each regular file as a stream, find FastCDC boundaries, and compute the full-file BLAKE3 hash. | Files are scanned concurrently when more than one worker is configured. |
| 5. Chunk hashes | Hash each chunk as it is read and retain its source range, size, and identity. | Hashing runs in the file worker; raw chunks are released immediately. |
| 6. Reuse decision | Compare each file with the previous manifest and look up already-known chunk identities. | Decisions are made per file and then combined. |
| 7. Dedupe planning | Keep only content not already represented in the repo and collapse duplicate new chunk identities. | Planning is deterministic and mostly sequential after collection. |
| 8. Artifact encoding | Group new chunks into bundles or encode loose compressed chunks, then write the artifact files. | Bundle groups and loose chunks are encoded concurrently. |
| 9. Manifest finalize | Sort entries, construct the snapshot blueprint/manifest, and persist the current repo metadata and indexes. | Final ordering and manifest construction are deterministic. |

Capture does not begin artifact encoding until the source scan and dedupe plan
identify the chunks that need to be written. This gives the packaging stage a
complete view of reuse across all files in the snapshot.

## Filesystem inventory and scan

Inventory traversal does not follow symbolic links. Regular files become file
work items. Directories and supported safe internal symlinks become metadata
entries for the snapshot manifest. Unsupported filesystem objects fail the
capture rather than being silently omitted. The source root itself is not
emitted as a snapshot entry.

Gutch uses the inventory's file size to choose a bounded in-memory FastCDC scan
for files no larger than the smaller of 1 MiB and the chunk profile's maximum
chunk size. Larger files use streaming FastCDC. Both paths use the resolved
chunking profile and produce the same boundaries and hashes. The small-file
read detects size changes and reuses the full-file digest for a single chunk.
The same read supplies the full-file BLAKE3 hash and the raw chunk boundaries.
Chunk hashes are calculated before the file result is returned, so
the manifest always contains ordered, content-addressed references rather than
only offsets.

The previous manifest is reusable for a path only when the current file has the
same normalized path, size, full-file hash, executable bit, and read-only bit.
When that comparison succeeds, Gutch reuses the previous file manifest without
rechunking it into new artifacts. Otherwise, each chunk is resolved through the
repo-wide chunk map; source ranges are retained only for chunks that are not
already available locally.

## Parallelization and resource limits

`RuntimeResourceHints.jobs` is the only worker-count input. With no override,
the runtime uses physical CPU count capped by process-visible logical
parallelism. An explicit value is clamped to at least one and at most the
logical-parallelism limit. Capture does not create an unbounded worker pool.

Parallel file scanning uses one-file scheduling units (`with_max_len(1)`) so a
batch of large neighboring files cannot remain assigned to one worker. Indexed
collection preserves inventory order, including each file's chunk order, before
deterministic deduplication and packing. Each file remains a sequential stream;
a single very large file can still determine the end of the scan stage.

The pool is reused across these units of work:

- metadata conversion for directories and symlinks;
- per-file streaming scans;
- pending chunk hashing within scanned files;
- bundle compression jobs; and
- loose compressed-chunk jobs.

Inventory enumeration, dedupe accounting, group assignment, and final manifest
ordering provide deterministic boundaries between parallel work. The scan
keeps the pending chunks needed for a file's hash phase in memory; the worker
count limits concurrent file work. Artifact encoding uses a shared working-memory admission budget. By default this
is 25% of physical RAM, bounded by the detected Linux cgroup memory limit when
present; failed RAM detection falls back to 512 MiB. Callers can override it with
`RuntimeResourceHints.memory_budget_bytes` or CLI `--memory-budget-mib`.
The resolved byte budget is included in `RuntimePlan` and the detailed trace's
`runtime.memory.budget` event. The caller reserves memory before dispatching
encoding jobs, so pool workers do not block on memory admission. Jobs reserve
raw input bytes plus 128 MiB for codec work. Result collection preserves input
order regardless of completion order. This budget does not cap process RSS or
change chunking, bundle grouping, or codec settings. A job exceeding the entire
budget fails with its required and configured byte counts; allocations are not
retried by halving the budget.

## Chunk and bundle locality

New chunks are first considered by content hash for compression probing. Before
bundle assignment, they are ordered by file-locality segment, file content
hash, file offset, and chunk hash. This keeps chunks from the same source-file
region close together when the bundle sizing policy groups them. Existing repo
chunks remain addressable through the chunk map and are not needlessly repacked
just to match a later locality order.

The resulting blueprint references chunk identities. The repo-wide chunk map
resolves each identity to a bundle or loose-chunk artifact. The selected
compression and storage profiles therefore affect artifact encoding and later
reading, while file identity remains based on raw content and manifest
properties.

## Timeline and diagnostic evidence

The capture profiler exposes the operation as `capture.build_snapshot`, with
coarse spans for inventory, source scanning, dedupe planning, artifact grouping,
and manifest construction. Detailed mode samples file scan and bundle jobs;
`capture.scan.file` includes chunk discovery, hashing, and result construction.
Capture timing results separate scan, file hash, chunk processing, dedupe,
artifact encoding, compression output, manifest finalization, and total wall
time. A trace shows overlap between file workers and artifact workers without
making worker-time sums look like elapsed wall time.

## Capture memory

Capture retains chunk identities and source ranges instead of all source bytes.
Duplicate chunks share one encoding plan. Payload memory scales with active file
workers and artifact group sizes; inventory and chunk-reference metadata still
scale with the dataset. Encoding re-reads source ranges, trading additional reads
for lower retained memory. Encoded bundle bytes stream directly to staged files.
Actual encoded size is checked against the preset maximum; oversized groups
are split at chunk boundaries. Raw groups are capped at 256 MiB. Both bundles and loose chunks are staged on disk and
released through the same operation-owned artifact lifecycle.

## Failure boundaries

- A path, unsupported object, read, chunk, or hash failure stops capture.
- A previous manifest is reused only after its file identity properties match
  and every referenced chunk has an available artifact. Missing artifacts are
  rebuilt from source, including when recapturing an identical snapshot.
- Encoding re-reads selected source ranges and verifies their chunk hashes.
  A changed or truncated source stops capture before snapshot publication.
- Corrupt or incomplete artifact writes do not become valid repo content.
- Manifest and artifact paths remain inside the derived repo layout.
- Capture does not publish authorization or remote URLs; remote synchronization
  consumes the completed local repo through its provider-neutral boundary.

## Implementation anchors

The owning implementation is `src/capture/scan.rs` for inventory and per-file
work, `src/capture/pipeline.rs` for runtime setup, dedupe, artifact grouping, and
manifest finalization, `src/artifact/chunk.rs` for streaming FastCDC, and
`src/runtime/mod.rs` for worker planning.

## Related pages

- [`03_snapshot_source_ingestion.md`](03_snapshot_source_ingestion.md) — source
  selection and snapshot identity before scanning.
- [`05_packaging_and_bundles.md`](05_packaging_and_bundles.md) — artifact and
  storage contracts after the scan plan.
- [`12_performance_and_development.md`](12_performance_and_development.md) —
  runtime overrides, profiling, and benchmark interpretation.

## Bundle sizing target and probe

The encoded target is the profile's preferred size, capped by its maximum.
It does not vary with estimated total snapshot size. The compression probe
estimates the raw capacity needed to approach that target; the final encoded
size check enforces the maximum and splits oversized groups.

The probe and bundle writer share the streaming encoder, including codec level,
Zstd window log 27, pledged input size, and frame finalization. The probe counts
encoded bytes without retaining compressed output and adds header, chunk-table,
trailer, and identity overhead using the bundle format's constants. For identical
chunk bytes, order, and boundaries, its estimated encoded size matches a real bundle.
Sampling remains approximate: the probe uses hash-ordered sample chunks, while
actual bundles use file locality and can have a different compression ratio.
