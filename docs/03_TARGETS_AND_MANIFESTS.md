# Gutch Blueprint: Targets And Manifests

Status: Directional, ready for first implementation pass  
Owner: Artifact Platform  
Last reviewed: 2026-05-02

## 7. Bundles / Containers

Chunks are the logical deduplication unit.

Bundles are the physical S3 object unit.

Gutch should not store every chunk as its own S3 object. Storage, request operations, listing, and provider-specific transfer costs still matter. Millions of tiny objects would increase operation count and hurt listing, planning, and download efficiency.

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

Bundle objects are immutable after publication. A build that discovers more chunks later must create additional bundle objects, not mutate an existing bundle.

### Initial Bundle Defaults

```text
bundle_max_mib = 32
bundle_preferred_mib = 16
bundle_format = gutch-bundle-v1
bundle_hash = blake3 over canonical bundle bytes
```

Bundle sizes are dynamic. The max size is a hard packing limit for normal bundles, while the preferred target gives the packer a normal size to aim for. Small updates may still create one bundle below the preferred target.

## 8. S3 Storage Layout

Suggested object layout:

```text
S3://gutch/
  v1/
    products/
      {product_id}/
        repos/
          {content_repo_id}/
            targets/
              {target_id}/
                manifest.json
                manifest.sig
            bundles/
              {bundle_id}.bundle
            metadata/
```

Rules:

- `v1/` is the artifact storage format version root.
- One `content_repo_id` path is one content repo within a product.
- One `targets/{target_id}/` path is one exact installable target identity within that content repo.
- `manifest.json` is the authoritative signed target description.
- `manifest.sig` is the detached signature for `manifest.json`.
- Bundles are content-repo-scoped shared payload containers, not target-scoped files.
- New target builds should reuse existing chunk locations when possible and create bundles only for chunks not already present in the content repo bundle store.

The backend should own S3 object key generation. Clients should not guess trust-sensitive paths.

## 9. Manifest Model

### Target

A target is one installable build identity. The `target_id` is a content-addressed hash derived from its tag key=value pairs:

```text
target_id = blake3(sorted(tag_key=val pairs))
```

### Target Manifest

The target manifest is the authoritative signed description of one target.

It should include:

```text
manifest_version
product_id
content_repo_id
target_id
target_tags
created_at
compression_profile
chunk_sizing_profile
bundle_sizing_profile
bundle_format_version
files
total_logical_size
total_compressed_size_estimate
signature_metadata
```

The target manifest must be signed. Clients should reject unsigned or invalidly signed manifests.

The target manifest is the single metadata JSON object for one target. It includes the per-file entries directly instead of referencing separate file-manifest JSON files.

`target_tags` is the sole carrier of target identity. There are no top-level `version_id`, `platform`, `architecture`, or `tier_id` fields — those values live exclusively as tag entries. Today the required tags are `version`, `platform`, and `arch`, with `platform=any` and `arch=any` allowed as explicit defaults for shared content.

### Sizing Profiles

Chunking and bundle sizing are separate profiles:

- `chunk_sizing_profile` controls FastCDC chunk sizes
- `bundle_sizing_profile` controls bundle packing sizes

Chunk sizing uses five presets:

- `tiny`
- `small`
- `balanced`
- `large`
- `huge`

Bundle sizing uses six presets:

- `tiny`
- `small`
- `balanced`
- `large`
- `huge`
- `massive`

| Preset | Chunk sizing | Bundle sizing |
|---|---|---|
| `tiny` | `8 KiB` / `32 KiB` / `128 KiB` | `8 MiB` max / `4 MiB` preferred |
| `small` | `16 KiB` / `64 KiB` / `256 KiB` | `16 MiB` max / `8 MiB` preferred |
| `balanced` | `16 KiB` / `64 KiB` / `256 KiB` | `32 MiB` max / `16 MiB` preferred |
| `large` | `32 KiB` / `128 KiB` / `512 KiB` | `64 MiB` max / `32 MiB` preferred |
| `huge` | `64 KiB` / `256 KiB` / `1024 KiB` | `128 MiB` max / `64 MiB` preferred |
| `massive` | `64 KiB` / `256 KiB` / `1024 KiB` | `256 MiB` max / `128 MiB` preferred |

The chunk sizing values are `min / avg / max`. The bundle sizing values are `max / preferred target`.

The selected presets are recorded separately in `chunk_sizing_profile.preset` and `bundle_sizing_profile.preset`.

### Auto Selection

`auto` is a chooser, not a stored artifact preset. It resolves to one of the supported presets before the manifest is written.

#### Chunk profile auto-selection

Chunk sizing uses the **median file size** as input.

| Median file size input | Resolved chunk profile | Chunk envelope |
|---|---|---|
| up to `32 KiB` | `tiny` | `8 KiB` / `32 KiB` / `128 KiB` |
| up to `256 KiB` | `small` | `16 KiB` / `64 KiB` / `256 KiB` |
| up to `2 MiB` | `balanced` | `16 KiB` / `64 KiB` / `256 KiB` |
| up to `8 MiB` | `large` | `32 KiB` / `128 KiB` / `512 KiB` |
| above `8 MiB` | `huge` | `64 KiB` / `256 KiB` / `1024 KiB` |

#### Bundle profile auto-selection

Bundle sizing uses the **total uncompressed target size** as input.

| Total uncompressed size input | Resolved bundle profile | Bundle envelope |
|---|---|---|
| up to `64 MiB` | `tiny` | `8 MiB` max / `4 MiB` preferred |
| up to `256 MiB` | `small` | `16 MiB` max / `8 MiB` preferred |
| up to `1 GiB` | `balanced` | `32 MiB` max / `16 MiB` preferred |
| up to `4 GiB` | `large` | `64 MiB` max / `32 MiB` preferred |
| up to `16 GiB` | `huge` | `128 MiB` max / `64 MiB` preferred |
| above `16 GiB` | `massive` | `256 MiB` max / `128 MiB` preferred |

For the requested product range of roughly `4 MiB` to `40 GiB`, this means `auto` can scale from `tiny` through `massive` without leaving the supported preset set.

`balanced` is the standard bundle profile and maps to a `32 MiB` maximum bundle size with a `16 MiB` preferred target size.

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
```

`file_offset` is not stored in the chunk reference. It is derived by accumulating `raw_size` in file order during reconstruction.

### Bundle Lookup

Bundle lookup uses the repo-level `ChunkIndex`:

```text
target manifest -> chunk_index.json -> bundle chunk tables
```

Each target manifest lists its required chunks through file entries. The chunk index at repo root maps `chunk_hash → bundle_id` for every chunk in the repo. The bundle's internal chunk table then resolves the exact location within that bundle.

Successful installs should not require a durable local chunk index or persistent local chunk payload cache by default.

### Tier Overlays

Tier overlays are deferred for now.

In v1, each installable target should be represented by its own complete target manifest under `targets/{target_id}/`.

If overlay-based tier modeling is adopted later, it should be added as a follow-up artifact model rather than assumed by the canonical v1 layout.

### Content Repo Manifest

Every content repo root contains `repo_manifest.json` (renamed from `repo.json`).

It identifies:

```text
manifest_version
product_id
content_repo_id
created_at
auto_tagging_rules
```

The `auto_tagging_rules` field replaces the earlier `target_schema`. It contains:

- `aliases`: `BTreeMap<String, BTreeMap<String, String>>` for normalizing tag values across synonyms (e.g. `win` → `windows`, `x64` → `x86_64`)
- `on_incomplete`: `Warn | Error` controlling behavior when required tags are missing from a target

### ChunkIndex

A repo-level `ChunkIndex` is stored at the repo root as `chunk_index.json`. It maps every chunk in the repo to its containing bundle:

```text
chunk_hash -> bundle_id
```

This enables efficient bundle lookup during reconstruction without scanning every bundle header. The index is rebuilt whenever chunks or bundles are added or removed.

### PluginInfo

Targets may include plugin metadata in the manifest:

```text
plugin_type: Mod | Dlc | LangPack | Plugin
dependencies: [plugin_id, ...]
overrides: [plugin_id, ...]
incompatible_with: [plugin_id, ...]
```

`plugin_type` classifies the target for install-time grouping and UI display. `dependencies` declares required plugins that must be installed alongside this target. `overrides` lists plugins whose files this target replaces at install time. `incompatible_with` lists plugins that must not be installed alongside this target.

Plugin resolution is handled by the dependency resolver.

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
