# Gutch Component

Status: Current

Scope: Gutch component current state

## Boundary

Gutch is a Rust engine for turning creator source trees into reusable,
verifiable repo artifacts and reconstructing exact local installs from
those artifacts. It owns repo layout, snapshot capture, chunking,
bundling, metadata and blueprint validation, reconstruction, verification,
repair, and repo synchronization mechanics.

Gutch does not own user identity, creator/product ownership, product
moderation, snapshot access, object-storage authorization, library membership,
background-job persistence, or application navigation.

Remote repo synchronization is provider-neutral at the engine boundary.
Callers provide a `RemoteRepositoryReader` for repo/snapshot lookup and a
`RemoteRepositoryWriter` for uploads and remote cleanup. Artifact sources may be
bytes or direct URLs. Gutch owns comparison, pull/push ordering, bounded
transfers, local repo mutation, progress, cancellation, and result types;
it does not attach backend bearer sessions or call host authorization routes.

The cross-crate Gutch surface is an operation-and-model façade. Callers use
preset-based build configuration and high-level snapshot, repo, remote,
verification, and repair operations. Resolved packaging profiles, chunk-index
maps, source-identity helpers and reconstruction workers remain engine-internal.
Current metadata-reader consumers use the supported repository path model and
guarded readers through `gutch::api`. Profiling mode/session
configuration and live feedback are public API contracts; trace files and
summaries are assembled inside Gutch.

Host adapters own remote routes, authentication, response decoding and translation
into provider contracts. Gutch owns generic remote command syntax and local
repository operations. See the [provider integration contract](10_cli_and_app_integration.md).
Tracked-repo config persistence receives an explicit path from the caller.
The [operation and module map](01_operation_ownership.md) records integration
boundaries and implementation ownership.

## Component Responsibilities

| Component | Handles | Authority and handoff |
|---|---|---|
| Gutch engine | Repo, snapshot, artifact, reconstruction, verification, repair, and provider-neutral remote synchronization | Compares and mutates local repos, invokes the provider contract, and returns typed results and progress; it does not authorize URLs |
| Gutch API | The sole cross-crate facade for typed operations and generic terminal parsing, implemented over private engine modules including `src/10_cli/` | Dispatches through `gutch::api::run_from_args`; remote commands require an injected provider context |
| Host adapter | Authentication, remote response translation and application state | Supplies provider implementations and authorized artifact sources |
| Host UI or CLI | Input, confirmation, progress and application lifecycle | Invokes Gutch and handles its final result |
| Remote service | Authorization and remote records | Enforces the host's access policy |
| Object storage | Artifact bytes | Stores and serves the supplied content |

## Interaction Flow

1. A creator supplies one or more extracted source trees and repo tagging
   rules.
2. Gutch resolves snapshot identity, scans supported entries, chunks file
   content, creates bundle objects, and writes repo metadata and binary
   indexes. Build callers select stable sizing, compression, and storage
   presets; the engine keeps the resolved profiles private.
3. App or CLI asks Gutch to preview, verify, fetch, reconstruct, or repair while
   rendering the same progress-event semantics. Remote command execution uses a
   provider context supplied by the caller.
4. The host resolves authorization and supplies a provider. Gutch does not
   derive access authority from local metadata.
5. Installation uses the snapshot-operations reconstruction pipeline to
   reconstruct into staging, verify the expected result, and promote it only
   after success.
6. App commits local install metadata and operating-system integration after
   Gutch reports successful completion.

A progress event describes operation phase and presentation detail. The final
`Result` remains the authority for success or failure.

## Core Terms

- **Repo** — a creator-managed Gutch workspace containing repo metadata, a
  repo-wide chunk index, snapshots, and content objects.
- **Snapshot** — one concrete installable tree identified by its content and
  supported filesystem properties; tags describe it separately.
- **Snapshot metadata** — lightweight JSON identity, capture, and tag state.
- **Snapshot blueprint** — the binary authority for filesystem entries,
  properties, hashes, and chunk references.
- **Snapshot product metadata** — optional snapshot-specific runtime and launch
  behavior.
- **Chunk** — a BLAKE3-addressed reusable piece of file content.
- **Bundle** — a content-addressed packaged group of chunks used by product
  delivery.
- **Install** — a verified local reconstruction of one snapshot.
- **Repair** — reconstruction of missing or corrupt expected content, with
  explicit handling of extra files.

## Current-State Pages

- `01_operation_ownership.md`
- `02_repository_snapshots_and_tags.md`
- `03_snapshot_source_ingestion.md`
- `04_scan_pipeline.md`
- `05_packaging_and_bundles.md`
- `06_reconstruct_pipeline.md`
- `07_verify_pipeline.md`
- `08_installation_verification_and_repair.md`
- `09_snapshot_runtime_metadata.md`
- `10_cli_and_app_integration.md`
- `11_path_safety_and_conflicts.md`
- `12_performance_and_development.md`
- `13_correctness_testing.md`
- [Current limitations](14_limitations.md) — unsupported capabilities, current
  boundaries, and established reasons.
- [Binary file layouts](15_binary_formats.md) — serialized fields, arrays,
  storage modes, and exact byte-size formulas.
