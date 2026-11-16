# Gutch Format Evolution

Status: Current

Scope: Versioning of stored repositories, documents, binary files and CLI output

Verified: 2026-11-16

## Four independent versions

| Version | Where | Changes when |
|---|---|---|
| Crate | `Cargo.toml` (`1.0.0`) | Any release; it says nothing about stored data |
| Repository format | `repo_metadata.json` `repository_format.version` (integer, `1`) | The set of files, documents or binary layouts a repository holds changes incompatibly |
| Document or binary version | `document_version` (`"1.0.0"`) in each JSON document; `version:u16 = 1` in each binary envelope or header | That document's or file's own encoding changes |
| CLI output | `output_version` (`"1.0.0"`) in every structured output | The machine-readable output contract changes |

A repository format names exactly one version of every document and binary file
it contains. Snapshot IDs use their own identity encoding
(`gutch-installation-id-v2`, see [repositories](repositories_snapshots_and_tags.md));
they never depend on how metadata or artifacts are stored, so a format change
never changes a snapshot ID.

## Stored JSON documents

Every stored document begins with fixed markers:

| File | `document_type` | `document_version` |
|---|---|---|
| `repo_metadata.json` | `gutch_repo` | `1.0.0` |
| `snapshots/<id>/snapshot_metadata.json` | `gutch_snapshot` | `1.0.0` |
| `snapshots/<id>/snapshot_product_metadata.json` | `gutch_snapshot_product` | `1.0.0` |
| `local_repo_config.json` | `gutch_local_repo_config` | `1.0.0` |

Readers check the markers before the exact parse, so a document from another
release fails with a clear format error instead of an unknown-field error. A
wrong `document_type` is a manifest error. Any other `document_version` is
`Error::UnsupportedFormat`; `needs_newer_gutch` is true when the found version
is a well-formed semantic version above the supported one.

User input files (`--constraints-file`, `--tags-file`, capture configuration)
are inputs, not stored documents, and carry no markers.

## Repository format and features

`repo_metadata.json` declares:

```json
"repository_format": {
  "version": 1,
  "features_required_to_read": [],
  "features_required_to_write": [],
  "optional_features": []
}
```

Opening a repository reads only this object first. A format other than `1`, or
a required feature this release does not know, fails with
`Error::UnsupportedFormat` (CLI `code: unsupported_format`, see
[CLI integration](../integration/cli_and_app_integration.md)). Read operations check the
read features; operations that change the repository also check the write
features before any change. Optional features may be ignored.

Features describe repository capabilities, such as an added file kind or binary
group; a changed document field changes that document's version instead. Feature
names are lower snake case, unique within the object, and never reused once
released, even after support for them ends. This release knows no features.

## Rules for future formats

- **Readers** support the current format and, from format 2 on, the previous
  major format. Older repositories need an upgrade with an older release first.
- **Writers** write the format the repository declares. New repositories get the
  newest format. A repository is never silently converted.
- **Upgrades** happen only through an explicit `gutch repo upgrade` command.
- **Additive changes** that old releases can safely ignore become optional
  features; changes old releases must understand become required features or a
  new format.
- **Derived files** (`chunk_map.gtcbin`) are rebuilt from bundles and blueprints
  rather than converted.
- **Frozen readers** for an older format live in `versions/v<N>/` modules beside
  the current code and are not changed after release.

During prerelease, format 1 is replaced directly; no compatibility readers
exist for superseded prerelease layouts.

## Not yet implemented

- The reader chain for older formats and `gutch repo upgrade`. They are needed
  only once format 2 exists.
- Remote format checks. Push and pull do not yet compare the remote repository's
  format with the local one.

## Schemas and frozen evidence

[`schemas/current/`](../../schemas/README.md) holds JSON Schemas generated from
the document types; a unit test fails when they drift. The parsers stay
authoritative for rules a schema cannot express. Each frozen corpus repository
under `tests/datasets/corpus/*/repository/format-v<N>/` is parsed by the current
readers in unit tests and exercised end to end by the frozen scenario
([correctness testing](../development/correctness_testing.md)).

`tests/harness_tests/test_release_freeze.py` compares the working tree with the
latest `v<major>.<minor>.<patch>` tag. A file changed, added or removed inside a
`schemas/v<N>/` or corpus `format-v<N>/` folder that the tag already contains
fails the test. New folders are allowed. Before the first release tag it skips.

## Release checklist

1. Confirm the crate, document, binary and output versions above are final.
2. Copy `schemas/current/` to `schemas/v<major>/` for each new major format.
3. Confirm the frozen corpus folders match the format being released.
4. Run the full checks, then tag the release (`v1.0.0` first).
