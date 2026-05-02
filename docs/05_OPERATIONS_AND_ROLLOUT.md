# Gutch Blueprint: Operations And Rollout

Status: Directional, ready for first implementation pass  
Owner: Artifact Platform  
Last reviewed: 2026-05-01

## 16. Fragmentation

Deduplication can scatter a target's chunks across many old bundles. This saves storage but can make reconstruction slower.

Track a fragmentation score per target:

```text
required_bundle_count
average_chunks_per_required_bundle
percentage_of_whole_bundle_downloads
range_read_count
estimated operation count
```

Future mitigation options:

```text
bundle locality scoring
packing chunks by target locality
limited duplicate chunk rewrite for hot targets
larger grouped bundle downloads for badly fragmented targets
```

Do not let v1 depend on compaction or bundle rewriting as routine maintenance.

## 17. Garbage Collection

Deleting a target removes target-level references. It does not immediately mean the referenced chunks or bundles can be removed.

Gutch should treat bundles as immutable in normal operation.

The default GC model is:

- derive chunk liveness from retained target manifests
- use chunk-to-bundle membership to determine bundle liveness
- delete only bundles whose stored chunks are no longer referenced by any retained target
- do not do per-chunk deletion
- do not rebuild bundles as normal GC

This means chunk liveness is a decision input, not a physical deletion unit.

Safe GC requires:

```text
manifest reference scan
chunk liveness map
bundle liveness map
index update
audit log
delete only after new indexes are durable
```

Practical GC rules:

- Keep bundles immutable after publication.
- A bundle is deletable only when every chunk it contains is no longer referenced by any retained target.
- A partially live bundle stays as-is.
- Target deletion may reclaim nothing if its chunks are still shared by other targets.
- GC should run at the bundle level, not by trying to remove dead chunks from existing bundles.

Initial v1 can defer GC automation, but the manifest and bundle formats must support target-driven bundle liveness checks.

## 18. Backend Responsibilities

The Go backend should handle:

```text
product metadata
version metadata
tier metadata
entitlement checks
artifact records
R2 object key ownership
bundle object ownership
upload orchestration
planner API
target manifest signing
retention policy
```

The backend does not need to reconstruct files, but it must understand target manifests, embedded file-manifest entries, chunk references, bundle references, and planning metadata.

## 19. Rust Engine Responsibilities

The Rust engine should handle:

```text
FastCDC chunking
BLAKE3 hashing
zstd compression/decompression
bundle creation
target manifest generation
local verification
staged reconstruction
safe promotion
repair
transient download and staging cleanup
update-size estimation
```

The engine should be usable both from the launcher and from a CLI.

## 20. Safety Rules

- Never modify the active install in place by default.
- Always reconstruct into staging first.
- Validate chunk hashes before use.
- Validate full-file hashes after reconstruction.
- Reject target manifest, size, hash, compression, or bundle mismatches immediately.
- Treat backend product, version, tier, and entitlement metadata as authoritative.
- Publish target manifests only after all referenced bundles exist.

## 21. Adoption Plan

### Phase 1 - Local Engine

Build Rust tooling for:

```text
FastCDC chunking
BLAKE3 hashing
zstd per-chunk compression
bundle packing
embedded file-manifest generation
target manifest generation
local reconstruction
local verification
```

### Phase 2 - R2 Storage

Add:

```text
R2 upload/download
bundle object layout
target manifest object layout
signed target manifests
```

### Phase 3 - Planner

Add planner support for:

```text
reuse from existing installed files
missing chunk calculation
whole bundle downloads
range reads
range coalescing
update-size estimates
```

### Phase 4 - Opt-In Products

Use Gutch for selected products first.


### Phase 5 - Default Artifact Path

Make Gutch default once it proves:

```text
reliable reconstruction
safe staged install
repair works under corruption
storage savings are meaningful
R2 operation count is acceptable
fragmentation is manageable
rollback works
```
