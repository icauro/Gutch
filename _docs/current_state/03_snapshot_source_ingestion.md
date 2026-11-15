# Gutch Snapshot Source Ingestion

Status: Current

Scope: Gutch component current state

## Boundary

Snapshot ingestion converts extracted source directories into Gutch snapshots.
One source is one filesystem tree; archives must be extracted before capture.
The source contract is shared by CLI capture, repo source synchronization,
the app Creator workspace, fixtures, and self-publish staging.

## Component Responsibilities

| Component | Handles | Authority and handoff |
|---|---|---|
| Creator, fixture, or staging tool | Extracted source trees and optional source-root configuration | Supplies paths and explicit tags without writing repo artifacts directly |
| Gutch source discovery | Auto or explicit source selection and path normalization | Produces an ordered set of contained source directories and config tags |
| Gutch tag resolution | CLI tags and config tags | Produces structurally validated snapshot tags; an empty set is valid locally |
| Gutch capture pipeline | Recursive scan, capture preview, and repo artifact construction | Writes the snapshot only after source validation succeeds; capture is additive and never replaces or deletes snapshots |
| App or CLI | Input collection, preview, confirmation, and result presentation | Sends absolute paths and renders Gutch's capture plan and result |

## Interaction Flow

1. Gutch loads `.gutch_source_config.json` from the source root or creates the
   default source-discovery skeleton on first use. This config never infers
   snapshot tags from source names.
2. The configured ingestion mode selects source directories.
3. Gutch resolves explicitly supplied tags in precedence order. The result may
   be empty for a local snapshot.
4. Capture preview reports the source path and tags. It never assigns a
   snapshot ID: content-derived identity is only available after the source
   scan. Equal tags across snapshots are valid and create distinct snapshots.
5. After confirmation where required, Gutch scans the tree and writes the new
   snapshot artifacts through the repo pipeline. Immutable artifacts and the
   chunk index are published before the snapshot manifest, which is the
   commit marker. If the computed snapshot ID already exists, capture
   returns `Unchanged` for identical metadata, or fails with
   `SnapshotMetadataConflict` or `SnapshotIntegrityMismatch` instead of mutating the stored snapshot; explicit retag targets
   one exact snapshot ID.

## Ingestion Modes

In `auto` mode, immediate child directories of the source root are snapshot
sources. `auto_ingestion_rules.ignore_paths` supports simple `*` and `?`
matching against child folder names. Folder names do not provide tags.

In `explicit` mode, only paths listed in
`explicit_ingestion_rules.snapshots` are sources, and they may be nested at any
depth. Each entry can provide tags. The list must be nonempty, duplicate
entries are rejected, and every selected path must exist as a directory.

Both mode-specific objects may remain in the config. Only the object selected
by `ingestion_mode` is validated. `.gutch_source_config.json` itself is outside
snapshot trees and is always ignored.

## Identity Resolution

Snapshot tags use `key=value`. Keys are ASCII lower-snake-case; each key has
one value. The remainder after the first equals sign is preserved. Values are
resolved in this order:

1. explicit CLI `--tag key=value` values;
2. tags on the matching explicit config entry;
3. no inferred or default values.

Later sources fill only unresolved tags; higher-precedence values win. An empty
result is valid only when repository constraints allow it. A host can require versions, matching key sets, or dates through repository
constraints.

## Path and Entry Rules

Explicit config paths are normalized once: leading `./` is removed,
backslashes become `/`, duplicate separators and bare `.` collapse, and
absolute paths or `..` segments are rejected. Every selected source remains
beneath its source root.

Each source is scanned recursively. Regular files, explicit directories, and
supported safe internal symbolic links become blueprint entries. There are no
implicit name-based ignores inside a selected snapshot source.

## Failure Boundaries

- Archives are not accepted as snapshot trees.
- An unlisted explicit-mode directory is not discovered implicitly.
- Renaming an explicit-mode folder does not change identity when its config
  entry and explicit tags are updated consistently.
- Source validation and capture preview occur before any repo write.
  Capture never deletes snapshots; callers delete exact snapshot IDs
  separately.
- Frontends do not reimplement tag parsing or containment checks.

The detailed scan, chunking, reuse, and capture parallelization stages are
documented in [`04_scan_pipeline.md`](04_scan_pipeline.md).
