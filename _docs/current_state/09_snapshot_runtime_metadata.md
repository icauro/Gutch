# Gutch Snapshot Runtime Metadata

Status: Current

Scope: Gutch component current state

## Boundary

Runtime behavior belongs to the exact snapshot that launches. Optional
`snapshot_product_metadata.json` stores direct or host-variant launch actions
and launch-time external dependency references separately from generic
snapshot identity and the filesystem blueprint.

There is no repo-wide launch profile, snapshot assignment layer, default
runtime, or reconstruction step. A snapshot without product metadata remains
valid generic Gutch content and has no product launch behavior.

## Component Responsibilities

| Component | Handles | Authority and handoff |
|---|---|---|
| Gutch | Runtime document parsing, schema validation, path validation, host-variant resolution, and canonical writing | Returns complete editable metadata or one resolved runtime for installation |
| App Creator workspace | Runtime-target and launch-action editing, clipboard transport, and save scope | Sends user intent and complete drafts through typed native commands; never owns runtime JSON rules |
| Backend | Independent per-snapshot product-metadata hash and artifact transport | Stores and delivers the optional artifact without merging it into generic metadata |
| App install lifecycle | Current-host resolution and offline launch-state persistence | Stores resolved launch actions in exact install metadata after successful installation |

## Runtime Validation and Resolution

Every executable and working-directory path is validated as a normalized path
inside the snapshot blueprint. Gutch rejects unsafe paths, duplicate action
identities, duplicate dependency identities, malformed variants, and runtime
documents with unsupported fields.

A document uses either direct all-host behavior or host variants. Installation
selects exactly one matching host variant when variants are present. Validation
rejects variants that one host could match together: every pair must require
different values for at least one shared tag key. Host tags
come from the app and are not written back into product metadata. Creator
editing always uses the complete raw document so inspection does not flatten or
discard variants intended for other hosts.

## Synchronization

Push and pull hash and transfer `snapshot_product_metadata.json` independently
for each snapshot. Removing the local optional file removes the corresponding
remote artifact on the next successful synchronization. Generic
`snapshot_metadata.json` never parses or serializes runtime state.

## Creator Presentation

The Creator workspace labels this concept **Runtime config**. The snapshot
table displays an abbreviated product-metadata hash, and snapshots sharing the
complete hash are highlighted together. The editor separates runtime targets,
Launch Actions, and Dependencies. Dependency editing is currently disabled.

Copy and paste use the complete canonical snake_case JSON document through the
typed native parser and serializer. Paste validates without automatically
saving. A creator may save only the selected snapshot or the selected snapshot
plus every snapshot currently sharing the same complete metadata hash.

## Failure Boundaries

- The app frontend does not resolve host variants or validate blueprint paths.
- A new snapshot version or tag does not automatically create a new runtime
  document.
- Editing one shared hash group is an explicit save-scope choice, not a
  repo-wide assignment.
- Runtime metadata does not define snapshot identity or filesystem content.
