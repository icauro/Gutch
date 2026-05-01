# How Gutch Works

Gutch is a versioned artifact system for game builds.

## Core idea

Gutch does not store version-to-version patch chains like:

```text
v1 -> patch -> v2 -> patch -> v3
```

Instead, each version is stored as:

```text
version manifest
-> file manifests
-> chunk references
-> bundles
```

That means every retained version is independently reconstructable.

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

### 5. Build file manifests

Each file gets a file manifest like:

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

If two files or two versions contain identical chunk bytes, they get the same BLAKE3 hash.

So Gutch only needs one logical copy of that chunk.

That is the main storage saving.

### 7. Compress each chunk

Each unique chunk is compressed with zstd.

Current default:

- zstd level `3`

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

### 9. Create the version manifest

The version manifest is the top-level description of a version.

It includes:

- product id
- version id
- platform
- architecture
- tier
- chunking profile
- compression profile
- list of file manifests
- required chunk set
- required bundle set
- logical size
- compressed size estimate

This is the main entry point for reconstruction.

### 10. Store artifacts

Conceptually, a version looks like:

```text
artifacts/v1/
  version.gutch.json
  file_*.json
  bundle-000.gutch.bundle
```

In production, these would be stored in R2.

## How reconstruction works

To reconstruct a version:

### 1. Load the version manifest

It tells Gutch what files should exist.

### 2. Load file manifests

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
- compare it to the file manifest

Only then is the file trusted.

## How version reuse works

Suppose:

- `v1` has chunks `A B C D E`
- `v2` has chunks `A B X D E Y`

Then:

- `A`, `B`, `D`, and `E` are reused
- only `X` and `Y` are new

So `v2` only needs:

- a new version manifest
- new file manifests
- bundles containing the new chunks

That is why later versions can behave like deltas even though the system is manifest-based.

## What "apply latest from v1 + deltas" means here

In the examples layout:

- `artifacts/v1` holds the full first build
- later artifact folders hold only new bundles for changed chunks
- reconstructing the latest version means:
  - use `v1` bundles for unchanged chunks
  - use later bundles for changed or new chunks
  - rebuild the latest files from the combined chunk pool

So it behaves like a delta system operationally, but structurally it is still:

- chunk store
- manifests over chunk references

## What Gutch uses today

- language: Rust
- chunking: FastCDC
- chunk hash: BLAKE3
- compression: zstd level 3
- storage unit: bundle
- manifests: JSON
- verification: full-file hash and chunk hash checks

## What is not fully implemented yet

The local engine works, but the full production system is not finished yet.

Not done yet:

- R2 integration
- signed manifests
- SQLite install state
- planner
- repair flow
- staged promotion
- proper multi-bundle splitting logic
- backend-managed delta publication rules

## Mental model

Shortest correct model:

```text
files
-> content-defined chunks
-> chunk hashes
-> compressed chunk blobs
-> packed into bundles
-> referenced by manifests
-> reconstructed and verified later
```
