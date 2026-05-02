# How Gutch Works

For canonical terminology and design rules, start with `01_GLOSSARY.md`, `02_FOUNDATIONS.md`, and `03_TARGETS_AND_MANIFESTS.md`.

Gutch is a target-based artifact system for game builds.

## Core idea

Gutch does not store version-to-version patch chains like:

```text
v1 -> patch -> v2 -> patch -> v3
```

Instead, each target is stored as:

```text
target manifest
-> embedded file manifests
-> chunk references
-> bundles
```

That means every retained target is independently reconstructable.

## Ground up

### 1. Start with a game build folder

Example:

```text
game/
  game.exe
  data/assets.pak
  audio/eng.pak
  textures/logo.tex
```

Gutch scans all files.

### 2. Read each file as bytes

For each file, Gutch:

- reads full file bytes
- computes a full-file BLAKE3 hash
- stores metadata:
  - path
  - size
  - mode
  - executable flag

### 3. Split each file into chunks with FastCDC

Gutch uses content-defined chunking.

That means chunk boundaries are chosen from the content, not fixed every 64 KB.

Why that matters:

- fixed-size chunking shifts badly when bytes are inserted near the start
- FastCDC usually keeps unchanged regions aligned
- unchanged content tends to keep the same chunk hashes across versions

Default chunking profile:

- min: `16 KiB`
- avg: `64 KiB`
- max: `256 KiB`

### 4. Hash each chunk

Each chunk is hashed with BLAKE3 over the raw uncompressed bytes.

That hash is the chunk identity.

Important separation:

- FastCDC decides chunk boundaries
- BLAKE3 identifies chunk content
- compressed bytes are not used as chunk identity

### 5. Build file-manifest entries

Each file gets a file-manifest entry like:

```json
{
  "path": "game.exe",
  "size": 123456,
  "full_file_hash": "...",
  "chunks": [
    {
      "chunk_hash": "...",
      "raw_size": 65536,
      "file_offset": 0
    }
  ]
}
```

This describes:

- which chunks make up the file
- in what order
- at what offsets

### 6. Deduplicate chunks

If two files or two targets contain identical chunk bytes, they get the same BLAKE3 hash.

So Gutch only needs one logical copy of that chunk.

That is the main storage saving.

### 7. Reuse or compress each chunk

Gutch checks whether each unique chunk already exists in the product bundle store.

If the chunk already exists, the new target manifest reuses the existing bundle location.

If the chunk is new, Gutch compresses it with zstd and packs it into a newly created bundle.

Current default:

- zstd level `6`

This is per-chunk compression.

### 8. Pack compressed chunks into a bundle

A bundle is a container file that stores many compressed chunks together.

Why bundles exist:

- object storage is inefficient with millions of tiny chunk objects
- bundles reduce request count
- bundles improve planning and download efficiency

Bundle contents:

- header
- chunk table
- compressed chunk payloads

Chunk table entries store:

- chunk hash
- raw size
- compressed size
- compressed offset
- compression type

### 9. Create the target manifest

The target manifest is the top-level description of one target.

It includes:

- product id
- version id
- platform
- architecture
- tier
- chunking profile
- compression profile
- embedded file-manifest entries
- required chunk set
- required bundle set
- logical size
- compressed size estimate

This is the main entry point for reconstruction.

### 10. Store artifacts

Conceptually, one exact target looks like:

```text
v1/products/{product_id}/targets/{version_id}/{platform}/{architecture}/{tier_id}/
  manifest.json
  manifest.sig
```

Its shared payload bundles live separately under the same product:

```text
v1/products/{product_id}/bundles/{bundle_id_prefix}/{bundle_id}.bundle
```

In production, these would be stored in R2.

## How reconstruction works

To reconstruct a target:

### 1. Load the target manifest

It tells Gutch what files should exist.

### 2. Load embedded file-manifest entries

They describe each file's chunk layout.

### 3. Read bundles

Gutch opens bundles, reads the chunk table, and extracts needed chunks.

### 4. Rebuild files

For each file:

- take chunk 1
- append chunk 2
- append chunk 3
- continue until the file is complete

### 5. Verify the result

After rebuilding:

- compute the full-file BLAKE3 hash
- compare it to the embedded file-manifest entry

Only then is the file trusted.

## How version reuse works

Suppose:

- target `v1` has chunks `A B C D E`
- target `v2` has chunks `A B X D E Y`

Then:

- `A`, `B`, `D`, and `E` are reused
- only `X` and `Y` are new

So `v2` only needs:

- a new target manifest
- updated embedded file-manifest entries
- existing bundles for reused chunks
- new bundles containing only `X` and `Y`

That is why later targets can behave like deltas even though the system is manifest-based.

## What "apply latest from v1 + deltas" means here

In the examples layout:

- `targets/.../manifest.json` holds the target metadata
- `bundles/...` holds the shared payload containers
- later target releases add a new manifest and only new bundles for changed chunks
- reconstructing the latest target means:
  - use older product bundles for unchanged chunks
  - use later bundles for changed or new chunks
  - rebuild the latest files from the combined chunk pool

So it behaves like a delta system operationally, but structurally it is still:

- chunk store
- target manifests with embedded file-manifest entries over chunk references

## What Gutch uses today

- language: Rust
- chunking: FastCDC
- chunk hash: BLAKE3
- compression: zstd level 6
- storage unit: bundle
- manifests: JSON
- verification: full-file hash and chunk hash checks

## What is not fully implemented yet

The local engine works, but the full production system is not finished yet.

Not done yet:

- R2 integration
- signed target manifests
- SQLite install state
- planner
- repair flow
- staged promotion
- range-read reconstruction from remote bundles
- persisted chunk-location index for faster remote planning
- backend-managed delta publication rules

## Mental model

Shortest correct model:

```text
files
-> content-defined chunks
-> chunk hashes
-> compressed chunk blobs
-> packed into bundles
 -> referenced by target manifests
-> reconstructed and verified later
```
