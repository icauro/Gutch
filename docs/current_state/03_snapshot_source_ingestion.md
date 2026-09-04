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
| Gutch identity resolution | CLI tags and config tags | Produces structurally validated snapshot tags; an empty set is valid locally |
| Gutch capture pipeline | Recursive scan, capture preview, and repo artifact construction | Writes the snapshot only after source and identity validation succeed |
| App or CLI | Input collection, preview, confirmation, and result presentation | Sends absolute paths and renders Gutch's capture plan and result |

## Interaction Flow

1. Gutch loads `.gutch_source_config.json` from the source root or creates the
   default source-discovery skeleton on first use. This config never infers
   snapshot tags from source names.
2. The configured ingestion mode selects source directories.
3. Gutch resolves explicitly supplied tags in precedence order. The result may
   be empty for a local snapshot.
4. Capture preview reports the destination slot and whether capture creates,
   relabels, or replaces repo state.
5. After confirmation where required, Gutch scans the tree and writes the new
   snapshot artifacts through the repo pipeline.

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

Tag values use `name:value` for scoped tags and `name::value` for exclusive
tags. Tag names are lower-snake-case and each name has one kind. Values are
resolved in this order:

1. explicit CLI `--tag key::value` values;
2. tags on the matching explicit config entry;
3. no inferred or default values.

Later sources fill only unresolved tags; higher-precedence values win. An empty
result is valid for local capture. The backend rejects a remote snapshot that
does not contain a non-empty `version` tag.

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
- Source validation and capture preview occur before repo replacement.
- Frontends do not reimplement tag parsing or containment checks.

The detailed scan, chunking, reuse, and capture parallelization stages are
documented in [`04_scan_pipeline.md`](04_scan_pipeline.md).

## Pinned Decisions

- `docs_agentic/pinned_decisions/spec_paths_and_ids.md`
- `docs_agentic/pinned_decisions/spec_tag_system.md`
- `docs_agentic/pinned_decisions/spec_version.md`
