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

On Windows, each inventory uses the metadata already obtained by directory
entry enumeration, avoiding a second metadata lookup per entry. The before/after
inventories remain independent fresh walks, and source-change verification stays
enabled. On other platforms, the entry API performs its normal metadata lookup.

## Execution timeline

The capture operation runs these stages in order. Parallel stages use the same
capture Rayon pool unless noted otherwise.

| Stage | Work | Parallel behavior |
| --- | --- | --- |
| 1. Source and identity | Resolve selected source roots, normalized paths, tags, and snapshot identity. | Performed before capture work begins. |
| 2. Runtime setup | Resolve `jobs` into a bounded worker count and create the capture worker pool. | One operation-local pool is shared by scan and packaging. |
| 3. Inventory | Walk each source with `WalkDir`, without following links, in filename order. Record regular files, directories, and supported symlinks. | Directory enumeration is ordered; metadata conversion can run on the pool. |
| 4. File scan | Read each regular file as a stream, find the selected CDC boundaries, and compute the full-file BLAKE3 hash. | Files are scanned concurrently when more than one worker is configured. |
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

Gutch uses the inventory's file size to choose a bounded in-memory chunk scan
for files no larger than the smaller of 1 MiB and the chunk profile's maximum
chunk size. Larger files use the borrowed chunk stream. Both paths use the resolved
chunking profile and produce the same boundaries and hashes. The small-file
read detects size changes and reuses the full-file digest for a single chunk.
The streaming adapter lends slices from its read buffer to the hashing step;
it does not allocate and copy an owned vector for each chunk. It retains unread
bytes until a refill requires compaction. Read-ahead capacity uses the existing
inventory size, stays between one and two maximum-sized chunks, and never changes
boundary decisions. This matches the former buffer-plus-owned-chunk worst-case
bound, although typical resident memory can increase slightly. Gutch owns a small
FastCDC v2020 kernel containing the default gear tables and level-1 boundary rules.
Plakar-compatible JC is selectable with `--chunker jc`. `--chunker vram256`
selects RAM with runtime AVX2 acceleration and an identical scalar fallback.
RAM searches for the maximum byte in its initial window, then cuts before the
first following byte at least as large. The preset is that window's size, with
four times that size as the maximum; it does not use Gear. Scalar and AVX2 both
persist `ram-v1`. CPU dispatch happens once per file. FastCDC remains the default.
The selected kernel is statically dispatched through the shared borrowed stream.
Upstream FastCDC is pinned as a test-only oracle for exact boundary comparisons.
The local subset retains its upstream license, copyright, and provenance.
The same read supplies the full-file BLAKE3 hash and the raw chunk boundaries.
Chunk hashes are calculated before the file result is returned, so
the manifest always contains ordered, content-addressed references rather than
only offsets.

The previous manifest is reusable for a path only when the current file has the
same normalized path, size, full-file hash, executable bit, read-only bit, and
newly scanned chunk hashes and lengths. Chunker or size changes therefore cannot
reuse a different old boundary layout in a new snapshot.
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

When sizing requires a compression probe, new chunks are first considered by
content hash. Before bundle assignment, they are ordered by file-locality segment,
file content hash, file offset, and chunk hash. This keeps chunks from the same source-file
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
- Each bundle-loading job retains only its current source file handle, reusing
  it for consecutive chunks from the same path. Switching paths closes the old
  handle; the job closes its reader before compression. Compression probes use
  the same reader. Handles and seek positions are never shared between workers.
  Loose-chunk jobs still open one source for their single payload. Readers reuse
  their known cursor position for contiguous ranges; unknown positions after I/O
  errors, gaps, backward reads and nonzero offsets on new handles seek explicitly.
- Bundle encoding takes ownership of each loaded byte buffer directly; it does
  not allocate and copy that payload again for shared ownership.
  A changed or truncated source stops capture before snapshot publication.
- Corrupt or incomplete artifact writes do not become valid repo content.
- Manifest and artifact paths remain inside the derived repo layout.
- Capture does not publish authorization or remote URLs; remote synchronization
  consumes the completed local repo through its provider-neutral boundary.

## Implementation anchors

The owning implementation is `src/capture/scan.rs` for inventory and per-file
work, `src/capture/pipeline.rs` for runtime setup, dedupe, artifact grouping, and
manifest finalization, `src/artifact/chunk.rs` for shared chunk scanning, and
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

The raw-capacity floor is the preferred bundle size, capped by the 256 MiB raw
bundle limit. When all unique new chunk bytes fit within that floor, planning
skips the compression probe and its hash sort: the estimate cannot change the
single planned group. Locality sorting still runs, using the floor as raw
capacity. Encoding still rereads and hash-checks source ranges, checks the final
encoded size including framing, and splits when necessary. Larger inputs retain
the normal sample and capacity estimate. Existing repo chunks are not repacked.

The probe and bundle writer share the streaming encoder, including codec level,
Zstd window log 27, pledged input size, and frame finalization. The probe counts
encoded bytes without retaining compressed output and adds header, chunk-table,
trailer, and identity overhead using the bundle format's constants. For identical
chunk bytes, order, and boundaries, its estimated encoded size matches a real bundle.
Sampling remains approximate: the probe uses hash-ordered sample chunks, while
actual bundles use file locality and can have a different compression ratio.


For uncompressed bundles, compression-ratio planning uses selected chunk lengths
and exact bundle metadata overhead. It does not load sample payloads. The sample
selection and estimated ratio are unchanged; reported probe-read bytes are zero.
For larger inputs, compressed codecs probe actual data. Packing loads and verifies
every selected source range, even when the size estimate needed no reads.
