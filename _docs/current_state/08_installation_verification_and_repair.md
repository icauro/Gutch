# Gutch Installation, Verification, and Repair

Status: Current

Scope: Gutch component current state

## Boundary

Gutch reconstructs one snapshot into a local filesystem tree, verifies that
tree against the snapshot blueprint, and repairs missing or corrupt expected
content. App install metadata and operating-system integration are committed
outside Gutch after the engine operation succeeds.

## Component Responsibilities

| Component | Handles | Authority and handoff |
|---|---|---|
| Backend delivery | Authorized snapshot metadata and artifact URL resolution | Authorizes the resolver request and returns current signed artifact URLs; it does not write local files |
| App install lifecycle | Destination selection, job identity, access revalidation, local metadata commit, and user repair choices | Supplies exact paths and artifacts to Gutch, then commits only a successful result |
| Gutch planner | Required artifact set, reusable local sources, runtime resources, and affected repair entries | Produces one execution plan for remote, cached, offline, update, and repair paths |
| Gutch reconstruction | Bounded direct-URL artifact download/reconstruction, decompression, hash checks, staging, and promotion | Produces a verified staged tree or leaves the prior committed install authoritative; it does not send backend session headers |
| Gutch verification/repair | Missing, corrupt, and extra classification plus selective rebuild | Returns classifications and a post-repair verification result |

## Interaction Flow

1. App resolves one authorized snapshot and destination install.
2. Gutch plans required artifacts and verifies reusable bundles, loose chunks, or
   unchanged source-install files before reuse.
3. Downloads and reconstruction run with bounded concurrency and may overlap.
4. Expected files are reconstructed into staging and hash-verified.
5. Verified staging is published using whole-directory replacement or selective entry replacement.
6. App records the committed snapshot and resolved launch state only after
   Gutch succeeds.

For remote fetches, Gutch bounds and writes supplied metadata, chunk-index,
product-metadata, blueprint, and artifact payloads atomically. Downloaded bundles
must match their expected BLAKE3 identity and trailing ID before cache promotion.
Cached bundles undergo the same identity check before reuse; corrupt entries
are removed so a later fetch can download a valid replacement. Filesystem errors
are returned without treating an unreadable entry as corrupt. Reconstruction
also validates framing and chunk content when reading the payload. A
transfer or validation failure fails the operation; the app keeps existing
installed content authoritative until a complete replacement succeeds.

For repair, Gutch first classifies the existing install, downloads only artifacts
needed by missing or corrupt files, promotes repaired expected entries, applies
the caller's explicit choice for extra files, and verifies the repaired expected
entries plus every requested extra-entry removal. Unaffected files retain the
successful classification from the repair plan and are not read a second time.

## Policies and change preview

`install_snapshot_from_repo` owns repository locking, destination guarding,
classification, source verification, planning, and execution. Reconstruction and
repair translate to these presets:

| Preset | Publication | Extra entries |
|---|---|---|
| Reconstruction | Complete staged directory replacement | Removed with the previous tree |
| Repair | Replace only missing/corrupt entries; update matching content's supported metadata in place | Preserved by default; remove with `--remove-extra-files` |

The API represents publication as `ReplaceDirectory` or
`Selective { extra_files }`. Extra-file policy is a choice only for selective
publication; complete replacement always replaces the entire directory.
Healthy files retain their
contents and modification times during repair. Damaged files are replaced in
full, not patched in place. Healthy source files can contribute reused chunks
when other files in the source installation are damaged.

`visit_installation_plan` and `installation preview` stream entry actions (`keep`,
`add`, `replace`, `remove`, `preserveExtra`, `updateMetadata`), reasons, artifact
availability, then a summary. Target paths, extra paths, and artifact IDs are each
sorted within their group. The summary records target ID, whether content was
inspected, retained bytes, verified source reuse, and logical staging bytes.
Staging bytes exclude filesystem allocation, backups, and metadata overhead;
this is not an available-disk-space check. Staged symlink dependencies may add
work even though healthy target entries remain in place.

The default preview hashes content. `--metadata-only` skips content hashes and
marks entries unverified; it cannot detect same-size content changes. Artifact
availability means the file exists, not that its bytes have been scrubbed.
Preview and repair share installation inspection, metadata-change classification
and affected-artifact planning. Execution consumes the inspection result built
for that operation. A standalone preview is a separate read-only operation;
a later execution builds its own plan under repository and destination locks.
These locks coordinate Gutch processes, not unrelated applications writing the
directory. No source monitoring is required.

## Verification Modes

Fast verification checks expected filesystem entries and file content hashes
and scans for extra files. Strict verification additionally checks chunk hashes.
Both return structured missing, corrupt, and extra issues. Gutch's CLI may
write an optional Perfetto trace and render its internal span summary, but
profiling details are not part of the cross-crate result contract.

Destination operations use the `gutch installation` CLI namespace:
`preview` reports proposed changes, `reconstruct` replaces the complete destination,
`verify` audits it, and `repair` rebuilds missing or corrupt entries. Explicit
destructive diagnostics use `debug corrupt-installation`.

An install with missing or corrupt expected content requires repair. Extra
files are reported independently. Repair can preserve extra files or remove
the classified extras when the caller explicitly requests full repair. Extra-entry
counters include directories. Removal preserves directories declared by the target
snapshot, even when deleting their last extra child.

## Reconstruction and Promotion

The canonical installation pipeline is shared by bundled and loose storage,
remote, cached, offline, update, and repair operations. Required files become
eligible for reconstruction as their artifacts become available. The runtime
planner uses bounded reconstruction workers, staging, and a shared bounded
output-handle cache. Bundled artifact jobs decode each required bundle once,
verify selected chunks, and fan them out to their target files.

Snapshot replacement preserves prior committed metadata and content
until the replacement verifies and promotion succeeds; once promoted, the
complete target tree replaces the old install, including removal of source-only
files and unrelated destination entries.

The execution graph, artifact/source reuse, output-handle cache, and remote
download overlap are detailed in
[`06_reconstruct_pipeline.md`](06_reconstruct_pipeline.md). Verification lane
scheduling and hash modes are detailed in
[`07_verify_pipeline.md`](07_verify_pipeline.md).

Selective repair stages and verifies required bytes before publishing entries.
Publication is not an atomic multi-file transaction. An I/O failure during
promotion can leave some entries repaired and others unchanged; retry inspects
that state and repairs what remains. Cancellation is deferred during publication,
but this does not turn failures into rollback. Directory replacement uses the
shared backup/recovery primitive in `filesystem/directory.rs`; repository snapshot
metadata uses that primitive too. Selective policy lives in `installation/publish.rs`.

## Failure Boundaries

- A download, decompression, chunk, file, or final verification failure is an
  operation failure.
- Pending or failed reconstruction does not become a launchable install.
- In-place repair deletes extra files only through the explicit full-repair
  choice; snapshot replacement uses a complete staged tree and therefore does
  not retain destination entries absent from the target blueprint.
- Repair classification cannot introduce paths absent from the authoritative
  snapshot blueprint.
- Local verification does not grant remote access or change backend library
  state.



Atomic metadata and artifact writes resolve their destination parent before
creating and promoting the temporary file. On Windows this retains the extended
path prefix, so snapshot metadata beyond the legacy 260-character limit can be
written and replaced. Failed writers still preserve the previous destination.
