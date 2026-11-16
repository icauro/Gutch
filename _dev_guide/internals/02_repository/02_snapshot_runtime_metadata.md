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
| Host editor | Runtime-target and launch-action editing | Sends complete drafts through `set_snapshot_product_metadata`; never owns runtime JSON rules |
| Remote service | Independent per-snapshot product-metadata hash and artifact transport | Stores and delivers the optional artifact without merging it into generic metadata |
| Host install lifecycle | Current-host resolution and offline launch-state persistence | Stores resolved launch actions in exact install metadata after successful installation |

## Runtime Validation and Resolution

Every executable and working-directory path is validated as a normalized path
inside the snapshot blueprint. Gutch rejects unsafe paths, duplicate action
identities, duplicate dependency identities, malformed variants, and runtime
documents with unsupported fields.

A document uses either direct all-host behavior or host variants. Installation
selects exactly one matching host variant when variants are present. Validation
rejects variants that one host could match together: every pair must require
different values for at least one shared tag key. Host tags
come from the host and are not written back into product metadata. Host
editors always use the complete raw document so inspection does not flatten or
discard variants intended for other hosts.

## Synchronization

Push and pull hash and transfer `snapshot_product_metadata.json` independently
for each snapshot. Removing the local optional file removes the corresponding
remote artifact on the next successful synchronization. Generic
`snapshot_metadata.json` never parses or serializes runtime state.

## Failure Boundaries

- Host user interfaces do not resolve host variants or validate blueprint paths.
- A new snapshot version or tag does not automatically create a new runtime
  document.
- Editing one shared hash group is an explicit save-scope choice, not a
  repo-wide assignment.
- Runtime metadata does not define snapshot identity or filesystem content.
