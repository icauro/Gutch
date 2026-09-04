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
| Gutch identity resolution | CLI tags, config tags, path inference, and schema defaults | Produces normalized snapshot tags and rejects a missing version |
| Gutch capture pipeline | Recursive scan, capture preview, and repo artifact construction | Writes the snapshot only after source and identity validation succeed |
| App or CLI | Input collection, preview, confirmation, and result presentation | Sends absolute paths and renders Gutch's capture plan and result |

## Interaction Flow

1. Gutch loads `.gutch_source_config.json` from the source root or creates the
   default auto-mode skeleton on first use.
2. The configured ingestion mode selects source directories.
3. Gutch resolves tags in precedence order and requires `version`.
4. Capture preview reports the destination slot and whether capture creates,
   relabels, or replaces repo state.
5. After confirmation where required, Gutch scans the tree and writes the new
   snapshot artifacts through the repo pipeline.

## Ingestion Modes

In `auto` mode, immediate child directories of the source root are snapshot
sources. `auto_ingestion_rules.ignore_paths` supports simple `*` and `?`
matching against child folder names. Tags may be inferred from folder names.

In `explicit` mode, only paths listed in
`explicit_ingestion_rules.snapshots` are sources, and they may be nested at any
depth. Each entry can provide tags. Name inference is disabled, the list must
be nonempty, duplicate entries are rejected, and every selected path must exist
as a directory.

Both mode-specific objects may remain in the config. Only the object selected
by `ingestion_mode` is validated. `.gutch_source_config.json` itself is outside
snapshot trees and is always ignored.

## Identity Resolution

Tag values use `name:value` for scoped tags and `name::value` for exclusive
tags. Tag names are lower-snake-case, each name has one kind, and `version` is
always exclusive. Values are resolved in this order:

1. explicit CLI `--tag key::value` values;
2. tags on the matching explicit config entry;
3. path-name inference in auto mode only;
4. defaults from the repo's tag schema.

Later sources fill only unresolved tags; higher-precedence values win. Capture
fails when `version` remains unresolved. No other tag is universally required;
defined tags may remain absent when they have no supplied, inferred, or default
value.

Auto inference splits the source name and matches repo-defined inference
prefixes. Scoped tags require their named scope prefix. The repo fixture
convention orders version, OS, architecture, then optional tier, but explicit
mode does not derive identity from folder organization.

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
  entry and tags are updated consistently.
- Source validation and capture preview occur before repo replacement.
- Frontends do not reimplement tag inference or containment checks.

The detailed scan, chunking, reuse, and capture parallelization stages are
documented in [`04_scan_pipeline.md`](04_scan_pipeline.md).

## Pinned Decisions

- `docs_agentic/pinned_decisions/spec_paths_and_ids.md`
- `docs_agentic/pinned_decisions/spec_tag_system.md`
- `docs_agentic/pinned_decisions/spec_version.md`
