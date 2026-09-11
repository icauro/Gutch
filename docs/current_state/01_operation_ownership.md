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
| Verify a reconstructed destination | Private `installation/` engine, exposed as `gutch::api::verify_installed_snapshot`; CLI `gutch reconstructed verify` | Supply the repo path, destination, mode, and runtime hints |
| Reconstruct and promote an install | Private `installation/` engine, exposed as `gutch::api::reconstruct_snapshot_from_repo` | Select the destination/staging paths and commit app metadata only after success |
| Repair a reconstructed destination | Private `installation/` engine, exposed as `gutch::api::repair_snapshot_from_repo`; CLI `gutch reconstructed repair` | Choose repair policy and extra-file handling; present the result |
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
