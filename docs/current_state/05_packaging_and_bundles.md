# Gutch Packaging and Bundles

Status: Current

Scope: Gutch component current state

## Boundary

Gutch scans snapshot file content into reusable BLAKE3-addressed chunks and
stores those chunks as either content-addressed bundles or loose compressed
chunk objects. Creators operate on snapshots; artifact placement is an engine
concern.

## Packaging Profile

Repo metadata owns the content packaging profile. The current bundled
profile uses FastCDC chunking, BLAKE3 chunk identity, configurable sizing
presets, and a generic compression adapter. The default chunk sizing preset
targets 128 KiB chunks, and the default bundle sizing preset targets 8 MiB
bundles. Runtime and benchmark overrides still use the same profile and build
pipeline.

The current compression adapters are:

| Algorithm | Accepted explicit levels | Default level | Output |
| --- | --- | --- | --- |
| `none` | No level is accepted | Not applicable | Raw bytes |
| `zstd` | `1..=22` | `3` | Compressed |
| `lz4` | `0..=9` | `0` | Compressed |

The algorithm and level are separate fields in the repo artifact profile.
Supplying a level outside the selected adapter's range, including any level
for `none`, fails profile validation. Compression and decompression dispatch
through the selected adapter; bundle framing and chunk identity remain
algorithm-neutral.

`bundled` is the current production product-delivery storage mode.
`loose_chunks` is supported by the engine's fetch, reconstruction,
verification, cleanup, and benchmark paths; it remains a diagnostic and
benchmark mode until the backend delivery contract publishes loose-chunk
URLs.

Build callers choose the chunk, bundle, compression, and storage presets. The
engine expands those presets into resolved profiles internally; callers do not
construct or depend on the profile implementation structs.

Compression kind is repository-wide because artifacts do not store it themselves.
Capture, fetch, and pull reject changing that kind in a populated repository;
use a fresh repository for a different algorithm. Chunking and compression can
vary between independently captured repositories without changing installation
identity. Existing complete blueprints remain reusable when chunk sizing changes.

## Bundle Contract

Bundle objects have extensionless content-addressed names under
`bundles/<first-two-hash-characters>/`. Their bytes contain the chunk table
and payload framing. They do not embed a magic value, compression profile, or
repo identity. The repo's
packaging profile supplies compression kind and compression unit when a bundle
is read.

Loose compressed chunks, when used, have extensionless hash names under
`chunks/<prefix>/`. The repo-wide `chunk_map.gtcbin` maps chunk identity
to the current local artifact location and begins with its own `GTCI` binary
header.

## Build and Delivery Flow

1. The scanner records supported filesystem entries and streams regular-file
   content through the configured chunker.
2. Existing chunk identities may be reused across files and snapshots.
3. The build pipeline stores required payloads as bundles or loose chunks
   according to the repo profile.
4. Snapshot blueprints reference chunk identities; the chunk map resolves
   each identity to its artifact kind and local path.
5. Fetch, reconstruction, and verification plan one generic artifact set and
   select the bundled or loose reader from that map.
6. Remote synchronization and backend storage reconciliation currently publish
   the bundled artifact contract; loose-object publishing is benchmark-only.

For installation delivery, the authorized app or CLI adapter resolves missing
artifact IDs to direct object URLs. Gutch consumes those URLs without backend
session headers and verifies each downloaded artifact against the repo content
identity. The benchmark CLI accepts the same mapping through
`--artifact-url-manifest` for presigned S3 URLs.

## Integrity and Failure Boundaries

- Chunk and bundle identities are content-derived rather than creator-chosen.
- Bundle and loose-chunk decoding requires the matching repo packaging profile.
- Corrupt framing, decompression failure, or hash mismatch fails the operation.
- A partially written download is not promoted as a final artifact.
- Metadata cannot redirect a chunk outside the fixed repo layout.

The capture-side ordering and artifact-grouping timeline is documented in
[`04_scan_pipeline.md`](04_scan_pipeline.md). The artifact-side execution,
cache, and remote-fetch behavior is documented in
[`06_reconstruct_pipeline.md`](06_reconstruct_pipeline.md).
