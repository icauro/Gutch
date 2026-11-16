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
repository operations. See the [provider integration contract](05_integration/01_cli_and_app_integration.md).
Tracked-repo config persistence receives an explicit path from the caller.
The [operation and module map](01_overview/01_operation_ownership.md) records integration
boundaries and implementation ownership.

## Component Responsibilities

| Component | Handles | Authority and handoff |
|---|---|---|
| Gutch engine | Repo, snapshot, artifact, reconstruction, verification, repair, and provider-neutral remote synchronization | Compares and mutates local repos, invokes the provider contract, and returns typed results and progress; it does not authorize URLs |
| Gutch API | The sole cross-crate facade for typed operations and generic terminal parsing, implemented over private engine modules including `src/10_cli/` | Dispatches through `gutch::api::run_command_line`; remote commands create their provider through the host's factory |
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

## Pages

Folder and page numbers give the reading order: the overview first, then the
topics in the order an operation runs.

- **Overview**
  - [Operation ownership](01_overview/01_operation_ownership.md) — operations,
    integration boundaries and the module map.
  - [Limitations](01_overview/02_limitations.md) — unsupported capabilities, current
    boundaries and established reasons.
- **Repository**
  - [Repositories, snapshots and tags](02_repository/01_repositories_snapshots_and_tags.md)
    — repository metadata, tags and snapshot identity.
  - [Snapshot runtime metadata](02_repository/02_snapshot_runtime_metadata.md) —
    runtime and launch behavior stored with a snapshot.
  - [Binary file layouts](02_repository/03_binary_formats.md) — serialized fields,
    arrays, storage modes and exact byte-size formulas.
  - [Format evolution](02_repository/04_format_evolution.md) — repository, document,
    binary and output versions, refusal rules and the release checklist.
- **Capture**
  - [Source ingestion](03_capture/01_source_ingestion.md) — source selection and
    snapshot identity.
  - [Scan pipeline](03_capture/02_scan_pipeline.md) — source inventory, chunking,
    reuse and capture parallelism.
  - [Chunkers](03_capture/03_chunkers.md) — selectable chunking algorithms and their
    comparison tools.
  - [Packaging and bundles](03_capture/04_packaging_and_bundles.md) — bundle and
    loose-chunk artifact contracts.
- **Installation**
  - [Reconstruct pipeline](04_installation/01_reconstruct_pipeline.md) —
    reconstruction jobs, staging, cache and remote fetch overlap.
  - [Verify pipeline](04_installation/02_verify_pipeline.md) — verification modes,
    hashing and worker scheduling.
  - [Verification and repair](04_installation/03_verification_and_repair.md) —
    installation policies, change previews and repair.
  - [Path safety and conflicts](04_installation/04_path_safety_and_conflicts.md) —
    path ownership, filename limits and conflict policies.
- **Integration**
  - [CLI and application integration](05_integration/01_cli_and_app_integration.md) —
    the CLI, the public API, providers and structured output.
- **Development**
  - [Performance and benchmarks](06_development/01_performance_and_benchmarks.md) —
    runtime planning, profiling and benchmark execution.
  - [Correctness testing](06_development/02_correctness_testing.md) — the test
    runner, scenarios and the frozen corpus.
  - [CLI builds](06_development/03_cli_builds.md) — release builds in GitHub Actions.
