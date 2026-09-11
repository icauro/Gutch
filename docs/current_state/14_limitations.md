# Gutch Current Limitations

Status: Current

Scope: Gutch engine and generic CLI capabilities

Verified: 2026-09-11

## Boundary

Gutch captures installable filesystem trees into deduplicated artifacts and
reconstructs verified destinations. It is not a general-purpose filesystem
backup service. This page records the limits of its current supported
operations and the reasons that can be established from their contracts.

Reasons have two meanings here:

- **Scope decision**: the capability belongs to a host or is excluded by the
  current installation contract.
- **Implementation boundary**: the current model or operation explains the
  limitation, but does not establish why the feature was omitted or its priority.

An implementation boundary is not a permanent design decision. Where no
separate rationale is recorded, this page does not infer that the feature is
unnecessary, too expensive, or intentionally postponed. It contains no promised
features or implementation roadmap.

Gutch already supports deduplication across files and snapshots, compression,
explicit directories including empty directories, safe internal symlinks,
artifact reuse, remote transfers, verification, repair, and orphan cleanup.
The limitations below qualify those capabilities rather than imply their absence.

## Repository Coordination and Recovery

### Coordination across processes

The engine does not acquire a repository-wide cross-process writer lock or
expose a repository lease contract. Its operation-local worker coordination
does not serialize independent CLI or host processes against the same repository.

**Current reason — implementation boundary:** writes use operation-owned staging
and individual atomic replacements. There is no engine-wide coordinator covering
capture, pull, deletion, and other repository mutations. A historical reason for
omitting that coordinator is not established.

Do not infer concurrent-writer safety from atomic file writes. Host coordination
may restrict overlaps, but it does not establish a standalone engine guarantee.
This limitation alone is not evidence that an existing host permits a corrupting
race.

### Repository-wide crash recovery

Snapshot directories are staged and promoted together. Repository metadata and
the chunk index are separate atomic files, not one transaction with that snapshot
directory. There is no public repository journal replay or rollback operation
that restores the complete repository to a selected pre-crash transaction.

**Current reason — implementation boundary:** the persistence boundary is an
artifact, metadata file, or snapshot directory. Normal failure cleanup and staged
promotion provide narrower protection than a repository-wide recovery protocol.
They do not establish recovery from every process-kill or power-loss boundary.

### Index recovery and missing payload

There is no supported operation that rebuilds a missing or damaged
`chunk_map.gtcbin` by scanning surviving bundles. Bundle inspection exposes chunk
tables, but does not rebuild and publish a repository index. Bundle decoding
also requires the repository compression profile.

**Current reason — implementation boundary:** reconstruction, repository
verification, and cleanup consume the existing index. Inspection and validation
do not form a recovery workflow. No separate reason for the absent workflow is
established.

Installed-file repair uses available artifact data; it is not repository salvage.
Recapture can rebuild missing artifacts from available source content. Neither
operation can recover bytes when every copy is lost. Gutch stores no
error-correcting parity. Chunk tables also do not replace snapshot blueprints:
payload identities alone do not recover the original file paths and tree.

## Storage Maintenance and Verification

| Limitation | Current reason and consequence |
|---|---|
| No compaction of partly live bundles | **Implementation boundary:** cleanup retains every artifact referenced by a surviving snapshot. It removes wholly unreferenced artifacts, but does not rewrite live chunks from partly unused bundles. Dead chunks remain stored while another chunk keeps the bundle live. No separate omission rationale is established. |
| Shared artifacts can be verified repeatedly | **Implementation boundary:** repository verification iterates snapshots and verifies each snapshot's required artifacts. A shared bundle can be read again for another snapshot in the same run. There is no operation-wide verified-artifact set in this path. |
| No budgeted or resumable repository scrub | **Implementation boundary:** verification selects one or all snapshots and a verification mode. It has no persistent coverage cursor, percentage selector, or byte/time work budget. Strict verification exists; scheduled coverage tracking does not. |
| No recompression operation for existing repositories | **Implementation boundary:** compression kind is repository-wide and supplied externally to artifact readers. Changing it in a populated repository is rejected. A fresh repository is required for a different algorithm; changing metadata alone does not convert stored bytes. |
| No engine disk-cache eviction policy | **Scope/implementation boundary:** the engine reuses local repository artifacts but does not classify a repository as a disposable cache or enforce a cache size budget. Whether another authoritative copy exists is host context. A creator repository must not be treated as disposable merely because the same format is also used for caches. |

Scrubbing means reading stored artifacts to check their integrity. A successful
fast check is not interchangeable with a strict payload check or a complete
reconstructed-tree verification. An artifact selected through several snapshots
still represents one stored object, even when it is checked repeatedly.

## Capture and Source Consistency

### Unchanged files still require content reads

Capture reads and chunks a file before comparing its full-file hash and supported
properties with previous manifests. Reuse avoids storing and encoding available
content, but does not skip that source scan through a timestamp/inode cache.

**Current reason — verified behavior:** reuse is based on content identity rather
than an assumption from file metadata. The capture regression test for changed
content with frozen modification time preserves this distinction. No claim is
made that the current implementation is the fastest possible way to retain it.

### No frozen live-tree capture

Gutch reads a supplied directory. It does not create filesystem snapshots,
coordinate database consistency, or read locked files through an integrated
volume-snapshot service. Re-reading newly encoded source ranges verifies their
chunk hashes, but does not freeze all files at a common instant.

**Current reason — scope decision:** ingestion accepts prepared, extracted source
trees. Preparing a stable source belongs to the caller. Source mutation detection
is a narrower integrity check than a point-in-time filesystem snapshot.

### Directory inputs and limited exclusions

Capture accepts directory sources, not archive extraction or a general stdin
stream. Source-discovery ignore patterns select immediate child directories;
they do not exclude arbitrary nested files inside an already selected snapshot.
Unsupported filesystem objects fail capture rather than being silently skipped.

**Current reason — current source contract:** one selected source is one
filesystem tree. Source discovery chooses trees, while scanning inventories their
supported entries. There is no separate recursive filtering or stream-ingestion
contract, and no broader omission rationale is established.

### No durable partial-capture checkpoint

Cancellation checkpoints test whether an operation should stop; they do not save
scan positions for another process. There is no public resume token for unfinished
capture or partially reconstructed staging. Failed reconstruction staging is
removed through the normal failure path. Transfer retries and reuse of completed
artifacts remain available, but are not a general restart protocol.

**Current reason — implementation boundary:** progress and cancellation belong to
the active operation. Complete artifacts can survive for reuse; incomplete
operation progress has no supported persisted representation.

## Filesystem Fidelity and Snapshot Identity

| Limitation | Current reason and consequence |
|---|---|
| No full backup metadata fidelity | **Current model:** files preserve executable/read-only state, not full Unix modes, ownership, per-entry timestamps, ACLs, extended attributes, or hidden attributes. Directories preserve read-only state. Those absent properties cannot be reproduced from the blueprint. The contract targets an installable tree; no individual omission rationale is established for every property. |
| No hard-link relationships | **Implementation boundary:** each regular-file entry has its own path/content references and no shared-inode identity. Equal files may deduplicate in storage but reconstruct as separate files. |
| No explicit sparse restoration contract | **Implementation boundary:** the blueprint has no hole extents and reconstruction has no sparse-output mode. Deduplicating or compressing zero bytes does not guarantee equivalent destination disk allocation. |
| Only contained symbolic links | **Scope decision:** link targets must be relative and remain within the snapshot. Arbitrary absolute or escaping links are incompatible with the contained installation path contract. Safe internal links are supported. |
| No independent historical record for every identical capture | **Scope decision:** snapshot identity describes the installed tree. Capture time, source location, tags, and packaging are excluded. Identical installed content reuses one ID rather than creating a new recovery point for each run. Metadata conflicts are handled separately; recapture is not unrestricted metadata replacement. |

## Inspection, Reconstruction, and Transfer

| Limitation | Current reason and consequence |
|---|---|
| No direct snapshot-tree diff or historical file search | **Implementation boundary:** the public tools list snapshots, inspect bundles, and compare physical paths. Pull differences describe synchronization artifacts. None is a two-blueprint file diff or search across stored file history. No separate omission rationale is established. |
| No arbitrary selected-path extraction | **Current operation boundary:** public reconstruction targets a complete snapshot. Repair selects damaged expected entries, but does not expose arbitrary file selection as an export operation. |
| No mounted snapshot filesystem | **Implementation boundary:** output is a reconstructed local directory. There is no virtual-filesystem or mounted random-read interface. No documented reason establishes whether such an interface is intentionally excluded. |
| No explicit transfer bandwidth ceiling | **Implementation boundary:** transfer policies bound concurrent workers and adapt to throughput/retries/backpressure. They do not impose a user-selected bytes-per-second ceiling. |
| Loose chunks are not a complete production publishing option | **Integration boundary:** local engine operations support loose chunks, while the current production delivery contract publishes bundled artifacts. Engine format support alone does not supply remote loose-object publication. |

Whole-snapshot reconstruction stages and verifies the replacement before
promotion. Its resulting tree excludes entries absent from the target snapshot.
Repair's explicit handling of extra files is a different operation and must not
be interpreted as selective export or preservation during snapshot replacement.

## Capabilities Outside the Engine

| Capability not provided by Gutch | Current reason |
|---|---|
| Backup scheduling, retention calendars, notifications, and arbitrary pre/post-capture scripts | **Scope decision:** Gutch executes operations; hosts own jobs and orchestration. Snapshot deletion accepts an exact ID rather than deciding business retention. |
| Built-in cloud credentials and account/access management | **Scope decision:** callers inject remote providers or authorized artifact sources. Gutch owns transfer and repository mechanics, not sessions, ownership, entitlement, or storage authorization. Provider injection is supported networking, not a built-in catalog of standalone storage accounts. |
| Server-enforced append-only or immutable retention | **Authority boundary:** preventing a writer from deleting remote history requires an enforcing provider/backend/storage policy. Local hashing and confirmation do not establish that remote guarantee. This says nothing about protection a particular deployment configures outside Gutch. |
| Client-controlled repository encryption | **Implementation boundary:** artifacts are compressed/content-addressed without an engine encryption or key-management layer. Hashing does not conceal bytes. Host authorization is separate; the engine ownership rules do not by themselves explain why artifact encryption is absent. No confidentiality guarantee from client-side repository encryption exists. |

## Evidence and Related Contracts

The supported surface is defined by [the public API](../../src/api.rs) and
[CLI arguments](../../src/command_api/args.rs). Scope boundaries follow
[Gutch ownership](../../AGENTS.md).

| Subject | Verification source | Owning behavior document |
|---|---|---|
| Coordination and atomic boundaries | [Capture](../../src/repo/capture_plan.rs), [remote synchronization](../../src/repo/remote.rs) | [Path safety and conflicts](11_path_safety_and_conflicts.md) |
| Recovery material and cleanup | [Bundle reader](../../src/artifact/bundle.rs), [cleanup](../../src/repo/cleanup.rs) | [Packaging](05_packaging_and_bundles.md) |
| Repository verification | [Repository verifier](../../src/repo/verify.rs) | [Verification](07_verify_pipeline.md) |
| Capture reuse and source rules | [Scanner](../../src/build/scan.rs), [capture tests](../../src/repo/capture_plan.rs) | [Scan pipeline](04_scan_pipeline.md), [source ingestion](03_snapshot_source_ingestion.md) |
| Filesystem properties and identity | [Entry model](../../src/manifest/content.rs), [identity](../../src/repo/source_identity.rs) | [Repository and snapshots](02_repository_snapshots_and_tags.md) |
| Reconstruction selection and restart boundaries | [Request types](../../src/snapshot_ops/types.rs), [transfer](../../src/snapshot_ops/transfer.rs) | [Reconstruction](06_reconstruct_pipeline.md), [repair](08_installation_verification_and_repair.md) |

This page describes engine limits. It does not certify all caller locking,
infrastructure durability, or power-loss behavior. Those guarantees require
evidence from the owning host and storage environment.
