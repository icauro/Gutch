# Gutch Bundles

For the canonical target and manifest model, start with `01_GLOSSARY.md` and `03_TARGETS_AND_MANIFESTS.md`. This document is the bundle-focused deep dive.

This document explains what a bundle is, why Gutch needs bundles, what a bundle contains, and how bundles should be created.

## What a bundle is

A bundle is the physical storage unit in Gutch.

Chunks are the logical deduplication unit.
Bundles are the files that actually store chunk data.

So:

- chunk = logical piece of content
- bundle = container that stores many chunks together

## Why bundles exist

If every chunk were stored as its own object in object storage, Gutch would create huge numbers of tiny objects.

That causes problems:

- too many object-store operations
- slower planning and listing
- poor download efficiency
- more request overhead

Bundles solve that by grouping many chunks into one object.

## Basic pipeline

Bundles should be created after chunking and after per-chunk compression.

The intended pipeline is:

```text
file bytes
-> FastCDC chunk boundaries
-> BLAKE3 hash each raw chunk
-> deduplicate by chunk hash
-> resolve existing chunk locations in the product bundle store
-> zstd-compress each new unique chunk
-> pack new compressed chunks into bundle(s)
```

Important:

- chunk identity is computed from raw chunk bytes
- chunks are compressed individually
- the whole bundle is not supposed to be compressed again as one stream

## What a bundle contains

A bundle should contain four conceptual parts:

1. header
2. chunk table
3. compressed chunk payloads
4. optional footer or checksum

### Header

The header identifies the file as a Gutch bundle and tells the reader how to parse it.

Typical fields:

- magic bytes
- bundle format version
- chunk count
- table length

### Chunk table

The chunk table describes every compressed chunk stored in the bundle.

Each entry should include:

- `chunk_hash`
- `raw_size`
- `compressed_size`
- `compressed_offset`
- `compression`

This lets the reader jump directly to a chunk without scanning the entire file.

### Payload area

The payload area stores the compressed chunk bytes.

Because the table records offsets and sizes, the reader can:

- read one chunk
- read a set of chunks
- range-read part of a bundle later in object storage

## Why chunks are compressed before bundling

Gutch compresses each chunk individually before placing it into a bundle.

That matters for two reasons.

### 1. Stable chunk identity

Chunk identity must stay stable even if compression settings change later.

If identity depended on compressed bytes, then changing:

- zstd level
- zstd dictionary
- bundle packing order

would change chunk identity even when the real content stayed the same.

That would break deduplication.

### 2. Random-access reads

If chunks are compressed independently, Gutch can later fetch exactly the compressed bytes for a specific chunk.

That is the basis for:

- partial bundle reads
- repair downloads
- efficient reconstruction

## Bundle creation goal

Bundle creation should choose a target compressed size for the current set of new chunks.

Blueprint default:

- max bundle size: `32 MiB` compressed
- preferred target bundle size: `16 MiB` compressed

That is a practical tradeoff:

- small enough to avoid huge waste when downloading
- large enough to avoid too many object-store objects
- flexible enough that small updates can produce one small bundle

In implementation, Gutch exposes five chunk sizing presets and six bundle sizing presets:

- `tiny`
- `small`
- `balanced`
- `large`
- `huge`
- `massive`

Each preset maps to a chunk sizing envelope and a bundle sizing envelope. The manifest records both chosen profiles so later tooling can understand how the artifact was produced.

If bundle sizing is selected automatically, the reference input is the logical size of the single target currently being built. It must not be derived from the sum of all retained targets already stored for the product.

| Preset | Chunk sizing | Bundle sizing |
|---|---|---|
| `tiny` | `8 KiB` / `32 KiB` / `128 KiB` | `8 MiB` max / `4 MiB` preferred |
| `small` | `16 KiB` / `64 KiB` / `256 KiB` | `16 MiB` max / `8 MiB` preferred |
| `balanced` | `16 KiB` / `64 KiB` / `256 KiB` | `32 MiB` max / `16 MiB` preferred |
| `large` | `32 KiB` / `128 KiB` / `512 KiB` | `64 MiB` max / `32 MiB` preferred |
| `huge` | `64 KiB` / `256 KiB` / `1024 KiB` | `128 MiB` max / `64 MiB` preferred |
| `massive` | `64 KiB` / `256 KiB` / `1024 KiB` | `256 MiB` max / `128 MiB` preferred |

The max size is a normal upper bound, not an exact output size. The final bundle may be smaller, and a hotfix with only a few new chunks may create a bundle below the preferred target.

## How bundles should be created

The correct algorithm is:

1. scan files
2. split into chunks
3. hash chunks
4. deduplicate chunks by hash
5. look up each unique chunk in the existing product bundle store
6. reuse existing bundle locations for chunks already present
7. compress only new unique chunks exactly once
8. append new compressed chunks into the current bundle until target size is reached
9. finalize that bundle
10. start a new bundle
11. continue until all new unique chunks are packed

In pseudocode:

```text
current_bundle = empty
current_size = 0
bundle_target_size = choose_target_size(total_new_compressed_bytes)

for each unique chunk:
  if chunk already exists in product bundle store:
    add existing bundle to target required_bundle_set
    continue

  compressed = compress(chunk)

  if current_bundle is not empty and current_size + compressed.size > bundle_target_size:
    finalize current_bundle
    start new current_bundle
    current_size = 0

  add entry to chunk table
  append compressed bytes to payload area
  current_size += compressed.size

finalize last bundle if not empty
```

Only newly introduced chunks should be written into newly generated bundles. Reused chunks should keep pointing at the bundle that already stores them.

`choose_target_size` should balance the new payload across a small number of bundles. For example, `12 MiB` of new compressed chunks can become one `12 MiB` bundle, while `180 MiB` can become three roughly `60 MiB` bundles instead of two full `64 MiB` bundles plus one tiny tail.

When the compression unit is `bundle`, packing should still aim at the preferred compressed bundle size. That means the planner should use an estimated compressed contribution for each chunk while filling a bundle, not raw input bytes alone, otherwise bundle-compressed output will undershoot the preferred size.

## What "finalize bundle" means

Finalizing a bundle means:

1. write header
2. write chunk table
3. write all compressed chunk payloads
4. compute bundle id
5. persist the bundle bytes

## Bundle identity

Bundle identity should be deterministic.

Blueprint direction:

```text
bundle_id = blake3(sorted chunk hashes + chunk sizes + format version)
```

Why deterministic identity matters:

- same logical bundle can be recognized again
- remote upload can skip existing bundles
- bundle reuse becomes possible across versions

## Example

Suppose Gutch has four unique compressed chunks:

- `A` = `40 KB`
- `B` = `55 KB`
- `C` = `20 KB`
- `D` = `70 KB`

If target bundle size is `128 KB`:

- bundle 1 stores `A + B + C` = `115 KB`
- bundle 2 stores `D` = `70 KB`

Bundle 1 table might conceptually say:

```text
A -> offset 0, size 40 KB
B -> offset 40 KB, size 55 KB
C -> offset 95 KB, size 20 KB
```

Then Gutch can read only chunk `B` by jumping directly to its byte range.

## What bundles are not

Bundles are not:

- patch files
- a second compression layer over the whole artifact
- target-specific by definition
- mutable append logs

A bundle may be referenced by many targets if the same chunks appear there.

Once published, a bundle should be treated as write-once. Later builds create more bundles for new chunks rather than editing old bundle bytes.

## Relationship between manifests and bundles

The target manifest does not contain file payloads.
It contains references.

The embedded file-manifest entries describe:

- which chunks make up each file

The bundle table describes:

- where those chunks physically live

So reconstruction is:

```text
target manifest
-> embedded file-manifest entries
-> chunk hashes
-> bundle lookup
-> read compressed chunk bytes
-> decompress
-> rebuild files
```

## Why bundle locality matters

Even with perfect deduplication, chunks for one target can become scattered across many bundles.

That creates fragmentation.

Fragmentation makes reconstruction slower because:

- more bundles are touched
- more range reads are needed
- request overhead grows

So a good future bundle strategy should also consider locality:

- keep chunks that are commonly used together near each other
- try not to scatter a single hot target across too many bundles

But Gutch does not need perfect packing to be useful in v1.

## What the current implementation does

Current local engine behavior:

- scans existing product bundles before building a target
- scans existing target manifests before building a target
- reuses previous file-manifest entries for unchanged files
- reuses existing chunk locations when a chunk hash is already present
- compresses only newly introduced unique chunks
- writes new chunks into content-addressed bundles up to the target compressed size
- stores a chunk table and payload area

Current limitations:

- existing bundle scanning is local and prototype-oriented
- there is not yet a persisted chunk-location index for faster remote planning
- reconstruction still reads whole local bundle files rather than issuing object-store range reads

## Practical rules

Bundles should be created so that:

- each unique chunk is compressed once
- bundle size is tracked using compressed sizes
- bundle target size is computed from the new compressed chunk volume
- chunks are packed until the dynamic target size is reached
- each chunk table entry points to one compressed chunk payload
- bundle ids are deterministic
- the whole bundle is not compressed again

## Short mental model

The shortest correct model is:

```text
chunks are the reusable data pieces
bundles are the files that store those pieces efficiently
manifests tell Gutch which pieces to pull from which bundles
```
