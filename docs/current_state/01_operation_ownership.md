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
| Verify Gutch repo artifacts | Private Gutch repo engine, exposed as `gutch::api::verify_gutch_repo_artifacts` | Supply the repo path, snapshot selection, and verification mode |
| Fetch snapshot artifacts | Private `transfer/` engine, exposed as `gutch::api::fetch_snapshot` | Resolve authorized artifact sources and provide progress/cancellation |
| Verify a reconstructed destination | Private `installation/` engine, exposed as `gutch::api::verify_installation`; CLI `gutch installation verify` | Supply the repo path, destination, mode, and runtime hints |
| Reconstruct and promote an install | Private `installation/` engine, exposed as `gutch::api::reconstruct_snapshot` | Select the destination/staging paths and commit app metadata only after success |
| Repair a reconstructed destination | Private `installation/` engine, exposed as `gutch::api::repair_installation`; CLI `gutch installation repair` | Choose repair policy and extra-file handling; present the result |
| Corrupt a reconstructed destination for tests/benchmarks | Private Gutch test/benchmark command operation; available through the CLI | Supply an explicit corruption plan and confirmation |
| Remote repo pull/push/link/bind | Private Gutch repo engine, exposed through `gutch::api` | The host adapter supplies the provider contract and authorized context |
| Install persistence and launch integration | Host application services and the GUI app layer | Own install records, job state, OS integration, and presentation |
| Command parsing and terminal output | Private `src/cli/`, exposed through `gutch::api` | The embedding CLI supplies top-level routing, endpoint, and session context |

The public `gutch::api` functions above are the cross-crate entrypoints. Internal
modules, worker functions, path helpers, and profiler recording machinery are
implementation details and should not be reimplemented by adapters.

Source navigation and import boundaries are maintained in the [module map](../../AGENTS.md).
The shared installation entrypoint is `install_snapshot_from_repo`; reconstruction
and repair are presets. `visit_installation_plan` streams change descriptions
from the same classification and job-planning functions.

Capture owns the derived state used to reuse existing chunks and previous file
manifests. Repository code owns persisted indexes and their codecs. Tag
normalization belongs to the tag model and is shared by API operations and CLI
parsing; engine operations do not depend on CLI helpers.

Local installation and combined fetch share source verification and reconstruction
preparation. Their callers retain repository coordination and destination guards
through execution and publication. Remote metadata publication remains owned by
the transfer operation.

## Public naming

Local-to-remote synchronization uses `push_repo`, `plan_push_with_provider`,
`RepoPushPlan`, `RepoPushSnapshotChange`, and `RepoPushStats`, matching `repo push`.
The corresponding download direction remains `pull_repo` and `RepoPullPlan`.

Artifact fetch inputs use `ArtifactDownload { artifact_id, url }`,
`ArtifactUrlProvider::resolve_artifact_urls`, and `SnapshotFetchRequest.artifacts`
and `.artifact_url_provider`. `fetch_snapshot_for_artifact_ids` can restrict the
fetch to selected bundle or loose-chunk IDs. Serialized downloads use
`artifactId` and `url`; CLI direct URLs use `--artifact ID=URL` or
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
