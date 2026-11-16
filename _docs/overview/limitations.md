# Gutch Current Limitations

Status: Current

Scope: Gutch engine and generic CLI capabilities

Verified: 2026-09-11

Gutch delivers installable trees; it is not a complete filesystem backup service.
It already supports deduplication, compression, empty directories, safe internal
symlinks, transfer, verification, repair, and orphan cleanup.

The reasons below describe current mechanisms or explicit scope boundaries.
Unless stated otherwise, why an absent feature was omitted is not documented;
its absence is not a permanent decision or a roadmap commitment.

## Repository Safety and Maintenance

Local reader/writer locks, recoverable metadata publication, and explicit
`repo index rebuild` now cover local coordination and index recovery.
Strict repository scrubbing verifies shared artifacts once per operation.
Lock acquisition polls cancellation for operations with a control hook. Remote
transactions, exhaustive power-loss testing, and resumable recovery are not provided.


| Limitation | Current reason and effect |
|---|---|
| No recovery of lost payload through parity | Repair needs healthy artifacts; recapture needs source content. No error-correcting parity is stored. Chunk tables cannot replace missing blueprints or recover original paths. |
| No partial-bundle compaction | Cleanup removes wholly unreferenced artifacts. One live chunk keeps its entire bundle, including dead chunks. |
| No budgeted/resumable scrub | Integrity checks select snapshots and a mode, without a saved coverage cursor, sampling selector, or byte/time budget. Fast checks, strict checks, and reconstructed-tree verification provide different coverage. |
| No existing-data recompression | Artifacts record their own codec, so new artifacts may use a different profile. Reused content is not automatically recompressed. |
| No disk-cache eviction budget | Gutch reuses repository artifacts without deciding whether they are disposable. Hosts know whether an authoritative copy exists elsewhere. |

## Capture and Filesystem Content

| Limitation | Current reason and effect |
|---|---|
| Unchanged files are still read and chunked | Reuse compares full content hashes, not timestamp/inode assumptions. Tests require detecting changed content with frozen modification time. |
| No frozen live-source capture | Callers supply prepared directories. Gutch does not create volume snapshots or coordinate databases; source-range hash checks do not freeze a whole tree at one instant. |
| Directory inputs only | Capture accepts extracted trees, without archive extraction or general stdin ingestion. Unsupported filesystem objects fail capture. |
| No recursive capture exclusions | Ignore patterns select immediate source directories, not nested files within a selected tree. |
| No durable operation resume | Cancellation checkpoints do not persist progress. Failed reconstruction staging is cleaned up; transfer retries and completed-artifact reuse do not resume an unfinished scan or staging tree. |
| Limited filesystem metadata | Files preserve executable/read-only flags; directories preserve read-only state. Full Unix modes, ownership, per-entry timestamps, ACLs, xattrs, and hidden attributes are absent from the model. |
| No hard-link relationships or sparse-output mode | File entries carry neither shared-inode identity nor hole extents. Deduplication does not preserve hard links or guarantee sparse destination allocation. |
| Only contained symlinks | The installation safety contract requires relative targets within the snapshot; absolute and escaping targets are rejected. |
| No recovery point for each identical capture | IDs describe installed content and supported properties, excluding capture time, tags, source location, and packaging. Identical trees reuse one ID; metadata conflicts remain separate. |

## Inspection and Delivery

| Limitation | Current reason and effect |
|---|---|
| No historical file search | Snapshot diff compares two stored trees; installation preview compares a target snapshot with a physical destination. Neither searches file history across all snapshots. Pull differences concern synchronization artifacts. |
| No arbitrary selected-path export or filesystem mount | Public reconstruction produces a complete local tree. Repair selects damaged entries, not arbitrary export paths; no mounted random-read interface exists. |
| No transfer bandwidth ceiling | Worker limits and adaptive backpressure control concurrency, not a bytes-per-second budget. |
| Loose-chunk publishing is limited | Engine operations support loose chunks; production remote delivery currently publishes bundled artifacts. |
| No client-controlled repository encryption | Artifacts are compressed and hashed without an engine encryption/key-management layer. Hashes do not conceal bytes; host authorization does not supply this guarantee. |

Selective repair is not an atomic multi-file commit; failure during publication
can require a retry. Indexes, manifests, and job plans still scale with repository
or snapshot size. Worker admission limits do not impose a fixed process-RAM ceiling.

Whole-snapshot replacement removes entries absent from the target tree. Repair
can handle extras separately; it does not change replacement semantics.

## Host Responsibilities

| Capability outside Gutch | Established boundary |
|---|---|
| Scheduling, retention calendars, notifications, and capture hooks | Hosts orchestrate jobs. The engine deletes explicitly selected snapshot IDs rather than deciding business retention. |
| Credentials, accounts, and access decisions | Callers inject remote providers or authorized sources. Gutch owns repository and transfer mechanics. |
| Remote append-only or immutable retention | Enforcement belongs to the provider/backend/storage policy. Local hashes and confirmations cannot prevent remote deletion. |

These limits do not establish what locking, durability, or storage protection a
particular host deployment provides.

## Evidence

- Surface and scope: [API](../../src/11_api.rs), [CLI](../../src/10_cli/arguments/mod.rs),
  [ownership](operation_ownership.md).
- Persistence and recovery: [capture](../../src/08_capture/execute.rs),
  [remote operations](../../src/08_sync/remote/mod.rs), [bundles](../../src/04_artifact/bundle.rs),
  [cleanup](../../src/07_repo/cleanup.rs), [path safety](../installation/path_safety_and_conflicts.md).
- Verification and capture: [repository verifier](../../src/07_repo/verify.rs),
  [scanner](../../src/08_capture/scan.rs), [source ingestion](../capture/source_ingestion.md).
- Filesystem and delivery: [entry model](../../src/05_manifest/content.rs),
  [identity](../../src/06_snapshot/identity.rs), [requests](../../src/09_installation/contracts.rs),
  [transfer](../../src/05_transfer/download.rs), [packaging](../capture/packaging_and_bundles.md),
  [reconstruction](../installation/reconstruct_pipeline.md), [repair](../installation/verification_and_repair.md).
