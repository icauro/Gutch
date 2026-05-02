# Gutch Blueprint: Targets And Manifests

Status: Directional, ready for first implementation pass  
Owner: Artifact Platform  
Last reviewed: 2026-05-01

## 7. Bundles / Containers

Chunks are the logical deduplication unit.

Bundles are the physical R2 object unit.

Gutch should not store every chunk as its own R2 object. R2 has no egress bandwidth fee, but storage and operations still matter. Millions of tiny objects would increase operation count and hurt listing, planning, and download efficiency.

A bundle stores many independently compressed chunks:

```text
bundle
  header
  chunk table
  compressed chunk data
  footer / checksum
```

Each chunk table entry should include:

```text
chunk_hash
raw_size
compressed_size
compressed_offset
compression
```

### Bundle Identity

Bundle identity should be deterministic and content-derived:

```text
bundle_id = blake3(sorted_or_ordered_chunk_hashes + chunk_sizes + format_version)
```

This allows bundle reuse across targets and lets upload skip bundles that already exist.

### Initial Bundle Defaults

```text
bundle_target_size = 16-64 MiB compressed
bundle_format = gutch-bundle-v1
bundle_hash = blake3 over canonical bundle bytes
```

The exact target size should be benchmarked with R2 request behavior, range reads, CDN/cache behavior, and real game build layouts.

## 8. R2 Storage Layout

Suggested object layout:

```text
r2://gutch/
  v1/
    products/
      {product_id}/
        targets/
          {version_id}/{platform}/{architecture}/{tier_id}/
            manifest.json
            manifest.sig

        bundles/
          {bundle_id_prefix}/{bundle_id}.bundle
```

Rules:

- `v1/` is the artifact storage format version root.
- One `targets/...` path is one exact installable target identity.
- `manifest.json` is the authoritative signed target description.
- `manifest.sig` is the detached signature for `manifest.json`.
- Bundles are product-scoped shared payload containers, not target-scoped files.
- `bundle_id_prefix` should be derived from the first bytes of the full content-derived bundle ID.

The backend should own R2 object key generation. Clients should not guess trust-sensitive paths.

## 9. Manifest Model

### Target

A target is one installable build identity:

```text
product_id + version_id + platform + architecture + tier_id
```

### Target Manifest

The target manifest is the authoritative signed description of one target.

It should include:

```text
manifest_version
product_id
version_id
platform
architecture
tier_id
created_at
chunking_profile
compression_profile
bundle_format_version
files
required_chunk_set
required_bundle_set
total_logical_size
total_compressed_size_estimate
signature_metadata
```

The target manifest must be signed. Clients should reject unsigned or invalidly signed manifests.

The target manifest is the single metadata JSON object for one target. It includes the per-file entries directly instead of referencing separate file-manifest JSON files.

### File Manifest

A file-manifest entry describes one logical file inside the target manifest:

```text
path
size
mode
executable
symlink_target_optional
full_file_hash
chunks[]
```

Each chunk reference should include:

```text
chunk_hash
raw_size
file_offset
```

### Bundle Lookup

For v1, the primary remote lookup model is:

```text
target manifest -> required_bundle_set -> bundle chunk tables
```

The manifest identifies which bundles may be needed for the target, and each bundle's internal chunk table resolves exact chunk locations within that bundle.

An additional derived chunk-to-bundle index may be added later for faster planning, but it is not required as part of the canonical v1 object layout.

Successful installs should not require a durable local chunk index or persistent local chunk payload cache by default.

### Tier Overlays

Tier overlays are deferred for now.

In v1, each installable target should be represented by its own complete target manifest under its exact target path.

If overlay-based tier modeling is adopted later, it should be added as a follow-up artifact model rather than assumed by the canonical v1 layout.

## 10. Local Install State

Use SQLite for local install metadata and staged-install state.

The database is a cache of what the launcher believes is installed. It must be rebuildable by scanning files and manifests.

Do not keep a persistent local chunk payload cache by default. After a successful install or repair, the launcher should keep the installed files and install metadata, then clean up temporary downloaded bundles, chunk ranges, and staging artifacts unless an explicit debug or operator mode says otherwise.

Suggested tables:

```sql
files(
  path TEXT PRIMARY KEY,
  size INTEGER NOT NULL,
  mtime INTEGER NOT NULL,
  mode INTEGER NOT NULL,
  full_hash TEXT,
  manifest_id TEXT NOT NULL,
  verified_at INTEGER
);

file_chunks(
  path TEXT NOT NULL,
  file_offset INTEGER NOT NULL,
  chunk_hash TEXT NOT NULL,
  raw_size INTEGER NOT NULL,
  PRIMARY KEY(path, file_offset)
);

active_install(
  product_id TEXT NOT NULL,
  version_id TEXT NOT NULL,
  tier_id TEXT NOT NULL,
  manifest_hash TEXT NOT NULL,
  activated_at INTEGER NOT NULL
);

staged_install(
  product_id TEXT NOT NULL,
  version_id TEXT NOT NULL,
  tier_id TEXT NOT NULL,
  staging_path TEXT NOT NULL,
  state TEXT NOT NULL
);
```

SQLite is useful because updates to install metadata can be transactional. If the launcher crashes or the machine loses power, the database should either represent the old known-good state or a recoverable staged state.
