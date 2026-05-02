# Gutch Blueprint: Pipelines And Flows

Status: Directional, ready for first implementation pass  
Owner: Artifact Platform  
Last reviewed: 2026-05-01

## 11. Upload / Build Pipeline

The build pipeline should be streaming and parallelized in Rust:

```text
read files
 -> FastCDC chunking
 -> BLAKE3 hashing
 -> dedupe lookup
 -> zstd per-chunk compression
 -> bundle packing
 -> R2 upload
 -> target manifest generation
 -> target manifest signing
```

The pipeline should not need to diff against every previous target. It only needs to know whether a chunk or bundle already exists.

### Upload Rules

- Skip chunks already known to exist.
- Skip bundles already known to exist.
- Create new bundles only for unique chunks not already present in the product bundle store.
- Upload bundles before publishing the signed target manifest.
- Never publish a target manifest that references missing bundles.

## 12. Download Planner

The planner should choose between:

```text
reuse chunk bytes already present in installed files
read chunks from existing installed files
range-read compressed chunks from bundles
download whole bundles
```

Planning should consider:

```text
missing logical bytes
missing compressed bytes
missing chunk count
required bundle count
range request count
nearby range coalescing
R2 Class B operation count
reusable installed-file hits
available disk space
decompression cost
fragmentation score
```

A smaller byte count is not always the best plan. If required chunks are scattered across too many bundles, downloading whole bundles may be faster and simpler than many tiny range requests.

### Range Reads

Because chunks are compressed independently, Gutch can range-read only the compressed byte spans needed from a bundle.

Rules of thumb:

```text
if most chunks in a bundle are needed:
  download whole bundle

if only a few chunks are needed:
  range-read those compressed chunks

if many tiny ranges are close together:
  coalesce ranges

if too many bundles are touched:
  consider larger grouped downloads
```

## 13. Install Flow

Default install mode is staged.

```text
1. Fetch and verify the signed target manifest.
2. Compare the target manifest with local install state.
3. Build download/reuse plan.
4. Download missing bundles or ranges.
5. Reconstruct target files into staging directory.
6. Verify every staged file against the embedded file-manifest entry hashes.
7. Promote staging to active install.
8. Record the active target in SQLite.
9. Clean up transient downloaded bundle/chunk data and staging artifacts after success.
10. Keep rollback metadata until success is confirmed.
```

Never modify the active install in place before staged validation succeeds.

## 14. Repair Flow

Gutch should support two verification modes.

### Quick Verify

Fast launch-time check:

```text
file exists
size matches
mtime matches if trusted
mode matches
install.db is internally consistent
```

This should be fast enough to run frequently.

### Full Repair

Explicit repair or recovery flow:

```text
scan install directory
hash files or chunks
compare against target manifest
identify missing/corrupt chunks
reuse valid local chunks
download only missing/corrupt data
reconstruct affected files in staging
verify before replacement
```

The install database should be rebuildable during full repair.

Downloaded bundles or chunk ranges may exist temporarily during repair, but successful repair should not leave behind a persistent local chunk payload cache by default.

## 15. Game Build Packaging Guidelines

Gutch works best when the game build pipeline is deterministic.

Avoid:

```text
one giant archive rebuilt every release
random asset order
embedded timestamps
random IDs
non-deterministic compression
repacking unrelated content
```

Prefer:

```text
stable archive order
stable metadata
split archives by content group
split language packs
split DLC / premium content
avoid touching unchanged files
reproducible build settings
```

For Unreal or Unity style builds, consider smaller logical packs:

```text
base_assets.pak
maps_forest.pak
maps_desert.pak
characters.pak
audio_en.pak
audio_tr.pak
dlc_pack_01.pak
```

CDC helps with changed offsets, but deterministic packaging improves reuse before Gutch even starts chunking.
