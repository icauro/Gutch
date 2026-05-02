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
-> zstd-compress each unique chunk
-> pack compressed chunks into bundle(s)
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

Bundle creation should try to pack many compressed chunks into a target compressed size window.

Blueprint default:

- target bundle size: `16 MiB` to `64 MiB` compressed

That is a practical tradeoff:

- small enough to avoid huge waste when downloading
- large enough to avoid too many object-store objects

## How bundles should be created

The correct algorithm is:

1. scan files
2. split into chunks
3. hash chunks
4. deduplicate chunks by hash
5. compress each unique chunk exactly once
6. append compressed chunks into the current bundle until target size is reached
7. finalize that bundle
8. start a new bundle
9. continue until all unique chunks are packed

In pseudocode:

```text
current_bundle = empty
current_size = 0

for each unique chunk:
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

A bundle may be referenced by many targets if the same chunks appear there.

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

- compresses unique chunks per chunk
- writes them into a single bundle in the current implementation
- stores a chunk table and payload area

Current limitations:

- only one bundle is created right now
- bundle sizing and packing still need improvement
- the implementation is not yet the final multi-bundle production design

## Practical rules

Bundles should be created so that:

- each unique chunk is compressed once
- bundle size is tracked using compressed sizes
- chunks are packed until the target size is reached
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
