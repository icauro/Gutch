# Gutch Operation Ownership

Status: Current

Scope: Gutch component current state

This table records the single implementation owner for each Gutch operation.
The GUI, CLI, and host adapters may collect input, provide context, map
results, and schedule work, but they delegate Gutch domain behavior to these
owners.

| Operation | Canonical owner | What adapters may do |
|---|---|---|
| Generic artifact build | Private `capture/` engine, reached through `gutch::api` or the CLI | Supply source paths, build presets, runtime hints, and progress sinks |
| Source discovery and snapshot capture | Private `capture/` engine, exposed through `gutch::api` | Supply source configuration, tags, and confirmation; present the plan and result |
| Repo layout and metadata | Private Gutch repo engine, exposed through `gutch::api` | Supply the repo root and persist only through Gutch operations |
| Repo status and advisory tag references | Private Gutch repo engine, exposed through `gutch::api` | Render the returned status and references |
| Verify Gutch repo artifacts | Private Gutch repo engine, exposed as `gutch::api::verify_repo` | Supply the repo path, snapshot selection, and verification mode |
| Fetch snapshot artifacts | Private `repo/` acquisition workflow, exposed as `gutch::api::fetch_snapshot` | Resolve authorized artifact sources and provide progress/cancellation |
| Fetch and reconstruct a destination | Private `installation/` workflow, exposed as `gutch::api::fetch_and_reconstruct_snapshot` | Supply authorized sources and destination/staging paths |
| Verify a reconstructed destination | Private `installation/` engine, exposed as `gutch::api::verify_installation`; CLI `gutch installation verify` | Supply the repo path, destination, mode, and runtime hints |
| Reconstruct and promote an install | Private `installation/` engine, exposed as `gutch::api::reconstruct_snapshot` | Select the destination/staging paths and commit app metadata only after success |
| Repair a reconstructed destination | Private `installation/` engine, exposed as `gutch::api::repair_installation`; CLI `gutch installation repair` | Choose repair policy and extra-file handling; present the result |
| Corrupt a reconstructed destination for tests/benchmarks | Private Gutch test/benchmark command operation; available through the CLI | Supply an explicit corruption plan and confirmation |
| Remote repo pull/push/link/bind | Private Gutch repo engine, exposed through `gutch::api` | The host adapter supplies the provider contract and authorized context |
| Install persistence and launch integration | Host application services and the GUI app layer | Own install records, job state, OS integration, and presentation |
| Command parsing and terminal output | Private `src/10_cli/`, exposed through `gutch::api` | The embedding CLI supplies top-level routing, endpoint, and session context |

The public `gutch::api` functions above are the cross-crate entrypoints. Internal
modules, worker functions, path helpers, and profiler recording machinery are
implementation details and should not be reimplemented by adapters.

The module map below records source navigation and import boundaries.
Artifact mechanics own packaging profiles; repository and snapshot schemas
consume those profiles without defining a separate codec policy.
The shared installation entrypoint is `install_snapshot_from_repo`; reconstruction
and repair are presets. `visit_installation_plan` streams change descriptions
from the same classification and job-planning functions.

Capture owns the derived state used to reuse existing chunks, including proportional encoded-byte estimates. Repository code owns
persisted indexes and their codecs. Runtime chunk-index entries retain physical
artifact references, encoded artifact sizes, chunk ordinals and raw chunk sizes.
Installation carries the complete encoded size once per artifact job and raw
content expectations per chunk. Tag normalization belongs to the tag model and
is shared by API operations and CLI parsing; engine operations do not depend on
CLI helpers.

Local installation and combined fetch share source verification and reconstruction
preparation. Their callers retain repository coordination and destination guards
through execution and publication. Remote metadata publication belongs to the repository acquisition workflow.

## Crate and Module Map

This project contains one Rust library crate, `gutch`. It captures files into
deduplicated snapshots, stores compressed artifacts, synchronizes repositories,
and fetches, reconstructs, verifies, and repairs snapshot content. Hosts call
`gutch::api`, including its generic CLI entry points; all modules below are
internal implementation owners.

| Module | Responsibility and features |
|---|---|
| `api` | Supported public types, operations, provider contracts, and CLI entry points |
| `cli` | Generic command parsing, family handlers, terminal progress, reporting, and the CLI's tracked-repository config file |
| `capture` | Source configuration and discovery, scanning, measurement, planning, packing, and capture execution |
| `installation` | Inspection, change previews, shared execution, selective publication, installation state, and combined fetch/reconstruction |
| `repo` | Layout, guarded readers, persistence, indexes, transactions, recovery, and cleanup |
| `sync` | URL-based snapshot fetch into a repository and provider-driven push, pull, and remote linking |
| `transfer` | Artifact URL contracts, HTTP streaming/retry, download scheduling, adaptive concurrency, and transfer feedback |
| `artifact` | Packaging profiles, chunking, hashes, full-file integrity, artifact references, compression, and repository-independent bundle/loose codecs |
| `manifest` | Repository/snapshot schemas, tags, launch metadata, and typed views |
| `snapshot` | Canonical content identity and snapshot comparison |
| `filesystem` | Atomic I/O, path safety, filesystem naming targets, locks, and recoverable directory replacement shared by repository and installation publication |
| `path_compare` | Physical tree comparison |
| `runtime`, `progress` | Resources, workers, cancellation, and progress |
| `profiling` | Diagnostic timings and traces selected by `--profile`; distinct from the packaging profiles in `artifact` |
| `error` | Typed failures and their payloads |

Internal callers import the owning module directly. Top-level modules form
numbered dependency layers: each module's layer is one more than the highest
layer its production code uses, so modules sharing a layer never use each other.
Each module lives at `src/NN_name` with its layer as the prefix, so a listing of
`src/` shows the dependency order; `lib.rs` loads it with `#[path]` because Rust
module names cannot start with a digit.

| Layer | Modules |
|---|---|
| 0 | `error` |
| 1 | `profiling` |
| 2 | `progress` |
| 3 | `filesystem`, `runtime` |
| 4 | `artifact` |
| 5 | `manifest`, `path_compare`, `transfer` |
| 6 | `snapshot` |
| 7 | `repo` |
| 8 | `capture`, `sync` |
| 9 | `installation` |
| 10 | `cli` |
| 11 | `api` (public facade) |

A module never uses its own or a higher layer; test code may. Typed failure
payloads carried by `Error` (path validation, directory promotion diagnostics
and capture outcomes) live under `00_error/` so `error` stays at the bottom.
`src/tests/module_boundaries.rs` reads each layer from its folder prefix and
checks it against the module's dependencies; a new top-level module or a
dependency that changes a layer must rename the folder, its `#[path]` and this table. Artifact codecs do not call
repository workflows; repository persistence does not call installation
orchestration. Repository artifact verification uses its own structure/identity
policy, independent of installation file/chunk inspection. Repository integrity
errors describe stored snapshot identity; capture translates them into its own
outcomes at the operation boundary. Transfer uses repository-independent artifact
references, but never acquires repository locks, loads/publishes snapshot
metadata, or drives installation execution. Combined fetch/reconstruction belongs
to installation and consumes `sync` fetch preparation under one write lock.
The public `api` module only re-exports owner operations and contracts; guarded
reader implementation belongs to `repo/read`. CLI handlers import the owning
modules directly; only hosts go through `api`.
Keep phase types with their owner and tests beside the code they exercise; a
separate test file is named after the file it tests (`x_tests.rs`, or
`mod_tests.rs` for a folder's `mod.rs`). Each source file opens with a `//!`
line naming its responsibility. Code imports the items it uses instead of
spelling `crate::` or `std::` paths inline. Functions with single-word names or
`std::fs` names are called through their module (`fs::write`, `output::line`),
and `io::Result`, `io::Error` and `fmt::Result` stay module-qualified so `Result`
and `Error` always mean the crate's own types. Platform-gated code keeps its
`std::os::*` paths inline.
Packaging profiles belong to `artifact/profiles`; codecs do not import manifest
schemas. Manifest validation may consume artifact profiles, hashes and codecs.
Capture groups source selection under `capture/sources` and artifact packing
under `capture/build`. Installation keeps its operations at the top level,
`plan` and `prepare` choose the work, and `installation/execute` owns staging
and publication: its `pipeline` coordinates artifact readiness and file
verification, `workers` owns memory admission, dispatch and terminal events, and
`job_io` reads, verifies and writes job data. Keep cancellation, draining and
handle-release ordering intact across these boundaries.

## Public naming

Local-to-remote synchronization uses `push_repo`, `plan_repo_push`,
`RepoPushPlan`, `RepoPushSnapshotChange`, and `RepoPushStats`, matching `repo push`.
The corresponding download direction remains `pull_repo` and `RepoPullPlan`.
Public identifiers spell the repository `repo` (`RepoConstraints`,
`RemoteRepoProvider`, `RemoteRepoService`, `Error::Repo`), and requests and
results name its directory `repo_root`. Operations borrow their requests,
paths, strings and `OperationControl`, and sync operations take `&dyn` provider
contracts. Previews are separate `plan_*` functions, such as
`plan_index_rebuild` beside `rebuild_index`, rather than boolean flags. The public metadata readers take the repo root and return
plain manifests; `RepoPaths` stays internal. `RepoStatus` reports the
directory as `repoRoot`; stored documents keep their field names.

Artifact fetch inputs use `ArtifactDownload { artifact_id, url }`,
`ArtifactUrlProvider::resolve_artifact_urls`, and `SnapshotFetchRequest.artifacts`
and `.artifact_url_provider`. Serialized downloads use
`artifactId` and `url`; CLI direct URLs use `--artifact-url ID=URL` or
`--artifact-url-manifest FILE`. The bundle-download endpoint adapter retains
its endpoint-specific `bundleId`/`downloadUrl` response mapping. Bundle counters
and `RemoteBundleSource` remain bundle-specific.

Destination operations use `installation preview|reconstruct|verify|repair`.
Explicit corruption diagnostics use `debug corrupt-installation`; API requests
and results use `InstallationCorruption*` and `corrupt_installation`.
repair and full reconstruction keep their distinct reports and policies.

Capture and repository summaries call the optional version label `version_tag`
(`versionTag` in camel-case reports). It is a descriptive tag, not snapshot
identity. Repair plans return `required_artifact_ids`, including bundle and
loose-chunk IDs, for use with artifact-filtered fetching.

These are prerelease source/CLI renames with no old-name aliases. Binary
identifiers and layouts remain v1. The API facade groups exports by capability;
its metadata loaders still acquire reader protection.

## Repository and transfer boundaries

Repository acquisition owns metadata validation, artifact selection and cache
publication. Transfer owns artifact URL input validation, bounded HTTP streaming,
retry, queues, adaptive concurrency and quantitative feedback. It does not acquire
repository locks or commit snapshot metadata. Installation owns combined remote
fetch and reconstruction and feeds required artifacts into the existing executor.

Combined fetch/reconstruction holds one repository write lock through preparation,
execution and snapshot publication. The destination guard is acquired after
snapshot preparation. Artifact readiness still enables reconstruction before all
downloads finish; repository metadata is published after successful execution.
Local reconstruction and repair continue to use the shared installation executor.

Repository artifact verification selects `RepoArtifactVerification::Fast`
(structure) or `Strict` (also content identity), reported as `fast` and `strict`.
The verifier loads repository metadata, the index and committed snapshots directly.
It decodes each blueprint once and retains that manifest through path and artifact
checks. Snapshot selection limits payload checks; metadata identity and chunk
references are still validated across all committed snapshots. Required artifacts
are checked by their index paths, with verification results cached across snapshots.
Installation verification independently selects full-file or additional chunk
checks. A stored blueprint whose content-derived identity differs from its
snapshot ID produces `SnapshotIntegrityMismatch`. Capture translates that
failure into its capture-specific integrity outcome.

The public API re-exports operations and models. Guarded metadata-reader
implementation belongs to the repository owner; both public metadata readers
acquire reader protection before loading committed data.
