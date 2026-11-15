# Gutch Verification Pipeline

Status: Current

Scope: Gutch component current state

## Boundary

The verification pipeline classifies a destination directory against a
snapshot blueprint. It reports missing, corrupt, and extra entries without
repairing them. Reconstruction and repair callers use the same verifier with
different path scopes; a verification result does not grant remote access or
mutate backend state.

## Execution timeline

| Stage | Work | Parallel behavior |
| --- | --- | --- |
| 1. Load authority | Read the snapshot manifest/blueprint and select `Fast` or `Strict` mode. | The manifest is the expected-state authority. |
| 2. Prepare paths | Choose a complete filesystem inventory or a targeted expected-entry resolver. | Targeted manifest path resolution runs on the verify pool. |
| 3. Validate containment | Canonicalize the root and check manifest paths and symlink-parent behavior. | Unsafe or invalid paths fail before file content work. |
| 4. Prepare expected files | Collect expected file metadata and sort files by descending size, then path. | Preparation is deterministic. |
| 5. Classify metadata | Check presence, entry type, read-only/executable properties, and size. | Non-file entries are classified before file tasks run. |
| 6. Verify file bytes | Fast mode checks the full-file hash; Strict mode checks the full hash and each blueprint chunk hash. | Files run through indexed parallel iteration with file-level work stealing. |
| 7. Classify extras | In complete-inventory mode, report filesystem entries absent from the blueprint. | Extra detection is part of inventory preparation, not a second file pass. |
| 8. Aggregate result | Merge issues and timing data and emit completion progress. | Indexed collection preserves deterministic sorted result order. |

## Complete versus targeted verification

`classify_directory` uses `scan_extra_files=true`. It walks the destination
without following links, builds a filesystem inventory, detects extra paths,
and augments the inventory for expected entries behind symlinked parents. This
is the full install-audit mode.

`classify_expected_entries` resolves only paths named by the blueprint. It does
not scan for extras and is used when a caller needs a focused classification,
such as post-repair verification of affected expected entries. Both modes use
the same metadata and content checks once a file is prepared.

## File checks and modes

Every expected file first receives metadata checks. A missing path produces a
missing issue. A wrong entry type, permission bit, or file size produces a
corrupt issue. Content reads and hash mismatches also become corrupt issues;
ordinary content failures are represented in the classification rather than
turning the whole verification into an unrelated fatal error. Cancellation and
path-safety failures remain operation errors.

Fast mode reads or maps the complete file and compares one full-file BLAKE3
hash. It does not validate individual chunk hashes.

Strict mode reads the file sequentially according to the blueprint chunk
 sizes. It updates the full-file hash, checks every chunk hash, and
confirms that chunk coverage exactly equals the expected file size. It therefore
detects a valid full-file byte stream whose stored chunk-level representation
does not match the blueprint.

Full-file hashing uses these current modes:

- Files below 16 MiB use a streaming reader with a 1 MiB thread-local buffer.
- Files at or above 16 MiB use serial mapped hashing when multiple files can
  occupy the verification pool.
- When the plan contains one file, or only one worker is configured, large-file
  verification uses Rayon-backed mapped hashing within the existing pool.
  This choice uses the prepared file count and requires no additional scan.

These choices change how bytes are read, not the BLAKE3 input or result.

## Parallelization and scheduling

Verification uses the same runtime worker policy as capture and reconstruction:
the default is physical CPU count capped by logical process-visible
parallelism, and `jobs` is bounded to the same limit. A supplied pool controls
the actual pool used by the operation.

Targeted path resolution uses parallel iteration over manifest entries. Full
inventory enumeration remains an ordered, no-follow filesystem walk because it
must establish one complete view and extra-path set before file work begins.

Prepared files are sorted by descending size, then path. An indexed Rayon
parallel iterator schedules individual files so idle workers can steal remaining
work. Indexed result collection preserves the prepared-file order without a
separate sort; completion order is concurrent. This is the
`largest_first_work_stealing` diagnostic schedule.

There is no parallel chunk-order read inside one Strict-mode file: that file's
chunks must be consumed in order to validate coverage and the full-file hash.
Files themselves remain parallel, and a large Fast-mode file can use the
selected mapped hashing mode.

## Remote target behavior

Verification itself is local. It reads the destination and local blueprint;
it does not download missing artifacts, call a bundle URL provider, or send
backend session headers. If a snapshot came from a remote fetch, remote
metadata and artifacts have already been resolved before this verifier runs.

The remote target therefore changes the preceding availability phase, not the
verification algorithm:

- A local or cached reconstruction is verified from local repo artifacts.
- A standalone remote fetch downloads artifacts first, then reconstruction and
  verification read the local results.
- Unified remote fetch-reconstruct overlaps downloads with reconstruction, but
  the verifier still runs against the staged local output after each file's
  producing jobs complete.

Gutch does not treat a remote URL, successful download, or local file presence
as an access decision. The caller's provider adapter supplies authorized
metadata and artifact locations; verification only checks bytes and filesystem
properties against the blueprint.

## Timing and progress evidence

Public verification entry points use the `installation.verify` marker
namespace. Trace spans distinguish path preparation, inventory/extra scanning,
file scheduling, and each file's metadata, read, hash, and chunk-hash work.
`VerifyTimings` separates runtime planning, preparation, stat, file reads and
bytes, full-file hashing and bytes, chunk hashing and bytes, file-verification
wall time, extra scanning, and total time.

Progress counts expected files. The final structured classification is the
authority for missing, corrupt, and extra results; progress events are only
operational reporting.

## Failure boundaries

- Invalid manifest paths, root containment failures, unsupported filesystem
  types, or cancellation stop the operation.
- Missing, wrong-type, wrong-permission, wrong-size, unreadable, or hash-
  mismatched expected files are returned as structured issues.
- Complete-inventory mode reports unexpected paths separately; it does not
  delete them.
- Verification does not repair, promote staging, remove extra files, or change
  repository contents.

## Implementation anchors

The owning implementation is `src/09_installation/inspect/mod.rs` for inventory, path modes,
lane scheduling, and classification, `src/09_installation/inspect/types.rs` for timing results,
and `src/04_artifact/integrity.rs` for full-file hash-mode selection.

## Related pages

- [`06_reconstruct_pipeline.md`](06_reconstruct_pipeline.md) — when file
  verification is released by reconstruction dependencies.
- [`08_installation_verification_and_repair.md`](08_installation_verification_and_repair.md)
  — install and repair lifecycle around classification.
- [`11_path_safety_and_conflicts.md`](11_path_safety_and_conflicts.md) — safe
  path resolution and destructive-action boundaries.
- [`12_performance_and_development.md`](12_performance_and_development.md) —
  profiling and interpretation of wall versus worker time.
