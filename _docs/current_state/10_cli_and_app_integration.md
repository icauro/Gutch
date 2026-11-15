# Gutch CLI and application integration

Status: Current

Applications invoke `gutch::api` and supply provider implementations for remote
operations. The standalone `gutch-cli` executable exposes local commands.
Host adapters own authentication, remote response translation, application
state, job scheduling and presentation. Engine behavior remains in Gutch. The public API re-exports owner operations;
repository metadata readers acquire coordination in the repository owner.

## Responsibilities

| Component | Owns |
|---|---|
| Gutch | Artifact mechanics, repository mutation, plans, results and progress |
| Provider | Authorized remote lookup, upload and deletion |
| Host | Input, confirmation, application persistence and operating-system integration |

The public API accepts operation requests, stable packaging presets, provider
contracts and progress/cancellation controls. Internal filesystem helpers,
resolved profiles and worker implementations are private.

Rust manifest chunk references use `ChunkHash` for `ChunkRef::chunk_hash`.
Hosts construct it with `ChunkHash::parse_hex` for text inputs or
`ChunkHash::from_blake3` for computed hashes. JSON still represents the field as
64 lowercase hexadecimal characters; malformed chunk hashes fail during JSON
deserialization. Blueprint and artifact binary layouts and snapshot identity
are unchanged.

Engine-owned HTTP downloads observe cancellation while waiting for response
headers and while reading either success or error bodies. The cancellation hook
is polled every 25 milliseconds during network waits; cancellation drops the
request and returns `Cancelled`, preserving the previous destination and removing
the partial temporary file. This polling interval is not a deadline for local
filesystem work. Provider callbacks and the public blocking-response helper keep
their own network timeout and cancellation responsibilities.

## CLI-to-API Contract

Command handlers parse arguments, request confirmation, adapt progress, render
reports, and map failed results to command errors. Engine operations own source
selection, tag merging, capture sequencing, profile persistence, and filesystem
behavior. The public API includes source resolution and batch capture, snapshot
estimation, bundle inspection, path comparison, corruption diagnostics, remote
listing, and tag-reference inspection alongside the main snapshot operations.

`capture_snapshot` persists the selected packaging profile after a
successful capture for every caller. Batch capture validates all source plans
before starting; successful earlier captures remain if a later capture fails.
It is not an atomic multi-snapshot transaction.

`run_from_args` returns failures to its embedding host. Failed path comparison
and repository verification write their requested reports and then return an
error; the executable maps that error to a nonzero exit status. Command handlers
do not terminate the host process.

Capture reports `required_bundle_count` as the number of distinct bundled
artifacts selected by the published chunk index for that snapshot. It includes
both generated and reused bundles once, excludes unrelated historical bundles,
and excludes loose-chunk objects. `generated_bundle_count` counts bundles
produced by that capture; CLI reused-bundle counts use the difference between
required and generated counts.

## Interaction Flow

1. CLI resolves relative paths against its invocation directory; API callers provide absolute paths.
2. A preview-capable operation asks Gutch for its plan before mutation.
3. The adapter renders risk and receives explicit confirmation where required.
4. The adapter invokes the same engine operation with a progress sink.
5. Gutch emits operation, phase, message, and quantitative progress events.
6. The adapter renders those events and treats the returned `Result` as final.

Snapshot reconstruction, verification, and repair all use the same active
`installation` pipeline. It verifies reusable source content, downloads or
reads required artifacts, applies filesystem metadata, and promotes a
completed staging directory. The CLI's `--artifact-url-manifest` option is a
caller-owned direct-URL bridge for presigned-object benchmarks; it does not
move S3 credentials into Gutch.

## Remote commands

Embedded hosts can invoke `run_from_args_with_remote_provider` for repo listing,
remote creation, binding, pull, push, unbinding and advisory tag references. Host-specific
options are resolved before invoking the generic parser. Credentials remain
with the provider; Gutch receives authorized operations and artifact sources.

## Command vocabulary

```text
repo init/status/rename/verify
repo constraints show/set/clear
repo index rebuild
repo remote list/create/bind/unbind
repo push/pull
repo tags references
snapshot capture/measure/list/diff/fetch/delete
snapshot tags replace
installation preview/reconstruct/verify/repair
bundle inspect
tree compare
debug corrupt-installation
```

`capture` stores source trees as snapshots. `fetch` downloads snapshot data and
artifacts into a repository without creating an installation. `installation
reconstruct` replaces the entire destination with a verified snapshot tree,
including removal of extra entries. Add `--fetch` to overlap supplied remote
artifact downloads with reconstruction; it requires `--snapshot-metadata-url`
and `--blueprint-url`. Remote source arguments require explicit `--fetch`.
`repo pull` synchronizes repository data and does not update installations.

`installation preview` reports proposed changes without applying them. Its
selective default previews repair; `--mode replace-directory` previews whole
reconstruction. Neither preview is a saved executable plan. `snapshot measure`
measures fresh-capture storage with the default packaging presets; it does not
account for repository reuse or estimate installation downloads.

`snapshot tags replace` replaces complete tag sets without recapturing content;
omitted keys are removed. `--snapshot ID --tag key=value` selects one snapshot;
`--tags-file FILE` supplies complete sets for an atomic bulk replacement.
Repository initialization and constraint replacement accept JSON through
`--constraints-file FILE`.

Capture reports use `repo_name` for the local display name. `remote_repo_id` is
present only when the repository has an actual remote binding. Reports do not
invent a product ID from the repository name.

Packaging presets control chunk sizing, bundle sizing, compression and storage.
Diagnostic profiling controls timing summaries and traces. Installation
verification hashes full files by default; `--strict` additionally validates
chunk hashes. `--metadata-only` applies to previews and skips content hashes.

## Compare stored snapshot trees

```text
gutch-cli snapshot diff --repo ABSOLUTE_REPO --from SNAPSHOT_ID --to SNAPSHOT_ID
gutch-cli snapshot diff --repo ABSOLUTE_REPO --from SNAPSHOT_ID --to SNAPSHOT_ID --json
```

The command compares two stored trees without reconstructing files or reading
payload artifacts. Default output lists changed paths with signed byte deltas
and a total. `--json` emits one object:

```json
{
  "changes": [
    { "path": "assets/new.png", "change": "added", "delta_bytes": 4096 },
    { "path": "config.json", "change": "modified", "delta_bytes": 0 }
  ],
  "delta_bytes": 4096
}
```

Changes are sorted by exact path. The change is `added`, `removed`, `modified`,
or `metadata`. File deltas are new size minus old size; added files contribute
their size and removed files its negative. Directories and symlinks contribute
zero bytes. Empty files and same-size content changes remain visible. Type or
symlink-target changes are `modified`; renames appear as removal and addition.
Content changes take precedence when properties change at the same time.

The diff excludes unchanged entries, packaging, tags and launch configuration.
It reports logical file sizes, not compressed storage or download sizes. JSON
deltas are exact signed integers; consumers must preserve integer precision
when reading values beyond their language's ordinary number range. Readable
paths are quoted and control characters escaped.

Differences and identical trees both exit successfully. Missing or invalid
snapshot metadata fails the command. The engine API is `diff_repo_snapshots`,
returning `SnapshotDiff`; both CLI formats render this same result.

## Filename compatibility policy

Create with optional targets, inspect, replace or clear the policy:

```text
gutch-cli repo init --repo REPO --filesystem windows-ntfs,linux-ext4
gutch-cli repo constraints show --repo REPO
gutch-cli repo constraints set --repo REPO --filesystem macos-apfs,ios-apfs
gutch-cli repo constraints clear --repo REPO
```

`constraints set --filesystem` replaces the complete target selection;
`constraints show` reads the policy and `constraints clear` removes it. No targets are enabled by default.
Policy updates fail if existing snapshots violate the new requirements.

Every operational command accepts `--error-json-out ABSOLUTE_FILE`. Structured
reports retain filename and directory-promotion failure details in this file.
The same report is included as `error_details` in failed `--json` results and
the final `--json-lines` summary; the existing `error` string remains readable.
Promotion failures use `code: directory_promotion_failed`. Details include
platform, UTC observation time, phase, elapsed milliseconds, the native I/O
error, and observations of staging, destination and backup paths. Rollback and
staging-cleanup attempts retain their own success or error results. Failed
metadata inspection is represented as unknown, with its inspection error.
Diagnostics are captured before rollback or subsequent staging cleanup;
observations of multiple paths are best-effort, not an atomic filesystem view.
Failure to save the error report is reported separately on stderr and never
replaces the original operation failure. No retries are added by diagnostics.

Filename failures return a nonzero exit status and write `code: filename_constraints`,
a concise message and structured `details` with the full violation list.
The public API exposes `RepositoryConstraints`, `FilesystemTarget`,
`read_repository_constraints`, `update_repository_constraints` and
`Error::PathValidation`. Consumers translate these types at their own boundary.

Other `Error` kinds name the failing domain: `InvalidInput` for caller
arguments and configuration, `Artifact` for malformed stored or received
artifact bytes, `Manifest` for snapshot manifest data, `Repository` for
repository state and synchronization, `Capture` for source capture, and
`Download`, `Verify` and `Reconstruct` for their operations. Structured kinds
include `Cancelled`, `IoContext`, `HashMismatch`, `InvalidPath`,
`SnapshotIntegrityMismatch` and `SnapshotCaptureConflict`.

## Output and previews

Every command supports readable output (default), `--json`, or `--json-lines`,
and `--output FILE` redirects the selected format. Progress and prompts use
stderr. JSON Lines ends with a `kind: summary` record; installation previews stream
entry events before that record. Single JSON collects preview events beside its
summary. Failures after dispatch return structured error information and a
nonzero status; parser failures use Clap diagnostics.

`--dry-run` supports repository init, rename, constraints, index rebuild, push
and pull; snapshot capture, delete and tag replacement; installation preview,
local reconstruction and repair. Reconstruction with `--fetch` rejects
`--dry-run` before downloading anything. Unsupported operations reject it before execution. Previews share
engine planning and execution policies, including staging and reuse paths.
They do not publish repository or installation changes. Explicit report/profile
outputs may still be written. Execution reinspects inputs; a preview is not an
executable transaction. `--yes` skips confirmation only.

Remote repository commands are `repo remote list/create/bind/unbind`. Creating
uses `--scope-id`; binding uses `--remote-repo-id`. Unbinding retains the remote
unless `--delete-remote` is explicitly selected. `repo rename --name` changes
only the local display name. `repo index rebuild` reports by default and needs
`--apply` to replace the index. Diagnostic corruption lives under
`debug corrupt-installation`. Retired command and flag names have no aliases.
