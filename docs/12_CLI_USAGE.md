# Gutch CLI Usage

For canonical terminology and core design rules, start with `01_GLOSSARY.md`, `03_TARGETS_AND_MANIFESTS.md`, and `04_DELIVERY_FLOWS.md`.

## Purpose

This document is the canonical usage guide for the `gutch` CLI.

Keep command-line usage, examples, and spec-file formats here instead of scattering them through design docs.

## Current Commands

- `gutch repo init`
- `gutch target insert`
- `gutch target export`
- `gutch target estimate`
- `gutch target_install verify`
- `gutch bundle inspect`
- `gutch path compare`

## 1. Repo Init

`repo init` creates a new content repo and populates it with an initial batch of targets.

It is batch-oriented by design:

- each target is scanned and chunked first
- chunk reuse is computed across the full batch
- bundling happens after the full batch has already been chunked

### Example

```text
gutch repo init \
  --product my-game \
  --content-repo base \
  --init-mode batch \
  --spec init-spec.json \
  --repo ./.content_repo
```

### Init Modes

`repo init` supports two initialization modes:

- `--init-mode batch`
  Chunk every target first, then dedupe and bundle across the full init batch.
- `--init-mode sequential`
  Treat init as a series of one-target-at-a-time commits in spec order.

### Spec File

The init spec is JSON.

Example:

```json
{
  "targets": [
    {
      "dir": "./builds/base-en",
      "tags": {
        "version": "1.0.0",
        "platform": "windows",
        "arch": "x86_64",
        "tier": "base",
        "language": "en"
      }
    },
    {
      "dir": "./builds/base-tr",
      "tags": {
        "version": "1.0.0",
        "platform": "windows",
        "arch": "x86_64",
        "tier": "base",
        "language": "tr"
      }
    }
  ]
}
```

### Output Layout

`repo init` writes directly into the repo root:

```text
{repo}/
  repo.json
  targets/{target_id}/manifest.json
  bundles/{bundle_id}.bundle
```

## 2. Target Insert

`target insert` creates one target from one source directory and inserts it into an existing content repo.

It reads product and repo identity from the repo root's `repo.json`.
The source directory defaults to the current working directory.
Optional reporting outputs are available:

- `--summary-out <path>` writes a plain-text command summary
- `--stats-json-out <path>` writes machine-readable JSON stats

### Example

```text
gutch target insert \
  --repo ./.content_repo \
  --dir ./builds/base \
  --version 1.0.0 \
  --platform windows \
  --architecture x86_64 \
  --tier base \
  --summary-out ./.artifacts/target-insert.summary.txt \
  --stats-json-out ./.artifacts/target-insert.stats.json
```

Or use the current directory as the source input:

```text
gutch target insert \
  --repo ./.content_repo \
  --version 1.0.0 \
  --platform windows \
  --architecture x86_64 \
  --tier base
```

## 3. Target Export

`target export` reconstructs one target from a content repo into an output directory.

Optional reporting outputs are available:

- `--summary-out <path>` writes a plain-text command summary
- `--stats-json-out <path>` writes machine-readable JSON stats

### Example

```text
gutch target export \
  --repo ./.content_repo \
  --target 1.0.0-windows-x86_64-base \
  --output ./reconstructed \
  --summary-out ./.artifacts/target-export.summary.txt \
  --stats-json-out ./.artifacts/target-export.stats.json
```

## 4. Target Estimate

### Example

```text
gutch target estimate \
  --dir ./builds/base \
  --product my-game \
  --version 1.0.0
```

## 5. Target Install Verify

`target_install verify` validates a real install directory against one target stored in a content repo.

Optional reporting outputs are available:

- `--summary-out <path>` writes a plain-text command summary
- `--stats-json-out <path>` writes machine-readable JSON stats

### Example

```text
gutch target_install verify \
  --repo ./.content_repo \
  --target 1.0.0-windows-x86_64-base \
  --dir ./install \
  --summary-out ./.artifacts/target-install-verify.summary.txt \
  --stats-json-out ./.artifacts/target-install-verify.stats.json
```

`target_install verify` currently reports:

- missing files
- corrupt files
- extra files

## 6. Bundle Inspect

### Example

```text
gutch bundle inspect --bundle ./.content_repo/bundles/abc123.bundle
```

## 7. Path Compare

`path compare` binary-compares two filesystem paths.

Supported shapes:

- file vs file
- file vs directory
- directory vs directory

When the expected side is a file and the actual side is a directory, `path compare` compares the file against `{actual}/{expected_filename}`.

Optional reporting outputs are available:

- `--summary-out <path>` writes a plain-text command summary
- `--stats-json-out <path>` writes machine-readable JSON stats

### Example

```text
gutch path compare \
  --expected ./truth \
  --actual ./reconstructed \
  --summary-out ./.artifacts/path-compare.summary.txt \
  --stats-json-out ./.artifacts/path-compare.stats.json
```

## Content Repo Manifest

Every content repo root contains `repo.json`.

It identifies:

- repo format/version
- product id
- content repo id
- created timestamp
- canonical `targets/` and `bundles/` subdirectories

## Notes

- `repo init` is the preferred entrypoint for first generation of a new content repo when multiple targets should be packed together.
- `repo init --init-mode sequential` is available when first generation should behave like repeated single-target commits instead of one batch pack pass.
- `target insert` is the single-target insert path for an existing repo.
- Manifest signing is still planned but not fully implemented in the CLI yet.
