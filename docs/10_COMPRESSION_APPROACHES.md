# Gutch Compression Approaches

For canonical terminology and current core design rules, start with `01_GLOSSARY.md`, `02_FOUNDATIONS.md`, `04_DELIVERY_FLOWS.md`, and `08_BUNDLES.md`.

## Purpose

This document compares two storage approaches for Gutch:

1. chunk-compressed bundles
2. bundle-compressed streams

This is a comparison and decision-support note. It explains the tradeoffs, edge cases, and where each approach fits best.

## Clarification: What Bundles Change

Bundles exist so Gutch does not store or fetch each chunk as its own object in S3 or R2.

That means the comparison in this document is not:

- chunk objects in object storage
- versus bundle objects in object storage

Gutch should use bundles in either approach.

The real comparison is:

- download whole bundle objects
- versus read byte ranges from inside bundle objects

And the compression question is:

- should the bytes inside a bundle be compressed chunk by chunk
- or should the bundle payload be compressed as one larger stream

## The Two Approaches

### Approach A: chunk-compressed bundles

Pipeline:

```text
file bytes
-> FastCDC chunk boundaries
-> BLAKE3 hash each raw chunk
-> deduplicate by raw chunk hash
-> zstd-compress each chunk independently
-> pack compressed chunks into bundle
```

Properties:

- dedupe unit: raw chunk
- compression unit: chunk
- physical storage unit: bundle
- direct-access unit inside bundle: compressed chunk span
- object-store fetch unit: still the bundle object

### Approach B: bundle-compressed streams

Pipeline:

```text
file bytes
-> FastCDC chunk boundaries
-> BLAKE3 hash each raw chunk
-> deduplicate by raw chunk hash
-> pack raw chunks into bundle payload
-> zstd-compress the bundle payload as one stream
```

Properties:

- dedupe unit: raw chunk
- compression unit: bundle
- physical storage unit: bundle
- direct-access unit inside bundle: usually the whole compressed stream or a large stream section
- object-store fetch unit: still the bundle object

## Important Separation

Dedupe does not require per-chunk compression.

The minimum dedupe requirement is:

- stable chunk boundaries
- chunk identity derived from raw bytes
- manifest and bundle metadata that can map chunk hashes to bundle membership

That means both approaches can still deduplicate by raw chunk hash across targets and versions.

## Shared Invariants

No matter which approach Gutch chooses:

- chunk identity must remain BLAKE3 over raw uncompressed chunk bytes
- compression settings must not change chunk identity
- bundles remain the object-store unit
- target manifests still reference chunk hashes
- bundles should remain immutable once published

## Approach A: Pros

- Easier partial access inside a bundle.
- A repair or update path can fetch only the compressed bytes for the needed chunks if range reads are supported later.
- A local reader can seek directly to one chunk span without decompressing unrelated chunk payloads.
- Corruption can often be isolated to a smaller payload unit.
- Chunk-level verification and extraction logic is more direct.
- Future planner optimizations remain available even if they are not implemented in v1.

## Approach A: Cons

- Compression ratio is usually worse than compressing one larger stream.
- Each chunk pays framing and reset overhead.
- Cross-chunk redundancy is lost because zstd does not see a larger shared window.
- Very small chunks can compress inefficiently.
- Build-time CPU cost can be higher because many independent compression jobs are created.
- Bundle tables must track a compressed offset and size for every chunk.

## Approach B: Pros

- Compression ratio is usually better, especially when neighboring chunks share repeated structure.
- Full-bundle download flow is simpler conceptually because the fetch unit and compression unit are the same.
- Metadata can be simpler because compressed offsets for every chunk may no longer be sufficient for direct extraction anyway.
- This model aligns well with a product choice of "bundles are small enough, so always download full bundles."
- It can reduce storage size and network bytes when full-bundle fetches are the normal path.

## Approach B: Cons

- Direct access to one chunk inside a bundle becomes harder or impossible without additional framing.
- Repair and near-version updates may download more bytes than strictly necessary.
- A client may need to decompress a full bundle stream even when it only needs a few chunks.
- Any future range-read planner becomes much less useful.
- Local extraction of one chunk from a bundle becomes more expensive in CPU and I/O terms.
- Corruption in a compressed bundle stream can affect a larger recovery unit.

## Practical Fit For Gutch

### Chunk-compressed bundles fit best when:

- partial repair matters
- near-version updates should minimize bytes fetched
- byte-range reads within bundle objects are planned now or likely later
- bundle sizes may grow
- remote planner flexibility matters more than best compression ratio

### Bundle-compressed streams fit best when:

- bundles are intentionally kept small
- clients are expected to download whole bundles anyway
- repair can also fetch whole bundles without concern
- simplicity and compression ratio matter more than selective access
- there is no real plan to support byte-range reads within bundle objects

## Edge Cases

### 1. Small bundles

If bundles stay near `4 MiB` to `16 MiB`, the benefit of fine-grained remote fetches drops.

Implication:

- bundle-compressed streams become more attractive
- chunk-compressed bundles still work, but some of their main network advantages are underused

### 2. Large bundles

If bundles grow toward `64 MiB`, `128 MiB`, or beyond, whole-bundle downloads become more expensive for repair and partial updates.

Implication:

- chunk-compressed bundles gain more value
- bundle-compressed streams create more download waste in fragmented cases

### 3. Mostly-correct local install with a few corrupt chunks

If a repair only needs a tiny fraction of one bundle:

- chunk-compressed bundles can recover with smaller payloads if byte-range reads within a bundle exist
- bundle-compressed streams usually force a larger download and larger decompression work

If Gutch intentionally downloads full bundles during repair anyway, this advantage weakens.

### 4. Hotfix with very few new chunks

A small hotfix may produce a tiny bundle.

Implication:

- whole-bundle fetching is not very wasteful
- the compression ratio difference between the two approaches may be small or may favor bundle compression slightly

### 5. Many tiny chunks

If chunk sizing shifts too small:

- chunk-compressed bundles pay more per-chunk overhead
- compression ratio can degrade noticeably
- dictionary experiments may help, but they add profile complexity

This is one of the strongest edge cases against per-chunk compression.

### 6. Large archive-like files with repeated structure

Game archives and packed assets often have recurring patterns across nearby regions.

Implication:

- bundle compression may capture more of that locality
- per-chunk compression may leave ratio on the table because the reset happens at every chunk boundary

### 7. Future CDN or object-store range support

Even if range reads are not needed now, they may become useful later for:

- repair optimization
- bandwidth-sensitive regions
- launcher resume/recovery behavior
- low-storage clients that avoid caching whole bundles

Approach A preserves that option more naturally than Approach B.

### 8. Corruption scope

With chunk-compressed bundles:

- a damaged compressed payload may affect one chunk entry or a small number of entries

With bundle-compressed streams:

- stream corruption can make a larger portion of the bundle harder to recover

The exact impact depends on framing and checksum choices, but the blast radius is usually larger for one large compressed stream.

### 9. Reused chunks across many targets

Both approaches preserve dedupe across targets because the dedupe key remains the raw chunk hash.

The difference is not whether reuse is possible.
The difference is how expensive it is to fetch and extract reused bytes from the bundle that stores them.

## Garbage Collection

Garbage collection has two separate layers:

1. logical chunk liveness
2. physical bundle liveness

This separation matters because Gutch deduplicates at the chunk level but stores data at the bundle level.

### Logical chunk liveness

A chunk is live if at least one retained target manifest still references its raw chunk hash.

Implications:

- deleting one target does not necessarily make its chunks deletable
- a chunk can stay live because another target still references it
- chunk liveness should be derived from retained manifests, not guessed from bundle membership alone

### Physical bundle liveness

A bundle is:

- fully live if all stored chunks are still referenced
- partially live if only some stored chunks are still referenced
- dead if none of its stored chunks are still referenced

Implications:

- dead bundles can be deleted directly after liveness metadata is made durable
- partially live bundles stay in place under an immutable-bundle GC policy
- chunk liveness informs bundle deletion decisions, but chunks are not deleted as independent stored objects

### Why GC is still tricky in a deduplicated bundle store

The hard problem is not finding dead chunk references.

The hard problem is:

- one bundle may contain chunks reused by many targets
- old and new targets may share only part of a bundle
- deleting one target often leaves many still-live or partially live bundles behind

That means Gutch needs both:

- manifest-to-chunk liveness
- chunk-to-bundle membership

### How the two compression approaches affect GC

#### GC under chunk-compressed bundles

Pros:

- chunk membership and verification stay explicit
- if Gutch ever introduces exceptional rewrite tooling later, per-chunk boundaries are already present
- immutable-bundle deletion still works cleanly because chunk liveness and bundle membership are explicit

Cons:

- there may be more metadata per chunk to track
- systems with many tiny chunks may still create many partially live bundles that remain undeletable for a long time

#### GC under bundle-compressed streams

Pros:

- dead bundles are still easy to delete
- if whole bundles tend to be either heavily live or fully dead, GC can stay simple for a while
- this fits well with a strict immutable-bundle policy

Cons:

- if a bundle is partially live, storage remains stranded until every referenced chunk in that bundle ages out
- any future exceptional rewrite tooling would be more awkward if bundle payloads are compressed as one larger stream

### Practical GC rules

- Never delete a bundle only because one target was deleted.
- Derive chunk liveness from the retained target-manifest set.
- Derive bundle liveness from chunk membership after chunk liveness is known.
- Delete only fully dead bundles directly.
- Keep partially live bundles unchanged.
- Do not treat chunks as independently deletable storage objects.
- Make new indexes durable before removing dead bundles.
- Keep an audit trail of bundle deletions.

### GC edge cases

#### 1. Target deletion with heavy cross-version reuse

Deleting an old target may reclaim almost nothing because most of its chunks are still referenced by newer targets.

#### 2. Many partially live bundles

This is the classic long-term dedupe problem.

Storage is not reclaimed until the remaining live chunk references in those bundles also disappear.

#### 3. Hot target rewrite pressure

If a frequently installed target is scattered across many aging bundles, locality may become a delivery concern even if GC intentionally avoids rewrites.

That is a separate packaging decision, not routine GC.

#### 4. Retention-policy changes

A large retention cleanup may create a sudden wave of dead and partially live bundles.

GC should handle this as a controlled background process that deletes only fully dead bundles.

#### 5. Product isolation

Because bundles are product-scoped, GC should run within one product's artifact namespace.

That keeps liveness decisions and audit history bounded to the product that owns the bundles.

## Decision Questions

These are the main questions to answer before locking the compression unit:

1. Will clients normally download whole bundles even for repair and hotfix cases?
2. How small will bundle targets stay in practice across real game builds?
3. Is future range-read support intentionally out of scope, or just postponed?
4. Is better compression ratio worth giving up cheap selective access inside bundles?
5. Which failure mode matters more: wasted bytes or added implementation complexity?
6. How much stranded storage from partially live immutable bundles are we willing to accept?

## Simple Rule Of Thumb

Use chunk-compressed bundles when:

- selective access is strategically important

Use bundle-compressed streams when:

- bundles are small and full-bundle fetch is the intentional steady-state model

## Summary

Both approaches keep chunk-based dedupe because dedupe depends on raw chunk hashes, not on the compression unit.

The real tradeoff is:

- `chunk-compressed bundles` optimize future selective access within bundle objects and repair flexibility
- `bundle-compressed streams` optimize simplicity and usually compression ratio when full-bundle download is acceptable

If Gutch is intentionally choosing small bundles and full-bundle downloads, bundle-compressed streams become a stronger candidate than they would be in a range-read-oriented design.
