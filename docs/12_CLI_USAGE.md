# Gutch CLI Usage

For canonical terminology and core design rules, start with `01_GLOSSARY.md`, `03_TARGETS_AND_MANIFESTS.md`, and `04_DELIVERY_FLOWS.md`.

## Purpose

This document is the canonical usage guide for the `gutch` CLI.

Keep command-line usage, examples, and spec-file formats here instead of scattering them through design docs.

## Current Commands

- `gutch repo create`
- `gutch repo discover-source-targets`
- `gutch repo inspect`
- `gutch repo list`
- `gutch repo schema-set`
- `gutch repo init`
- `gutch repo delete-target`
- `gutch target insert`
- `gutch target export`
- `gutch target estimate`
- `gutch target publish`
- `gutch target_install fetch`
- `gutch target_install apply`
- `gutch target_install verify`
- `gutch bundle inspect`
- `gutch path compare`

## Target Identity Normalization

Gutch normalizes target identity values before writing a manifest.

`version` is always required.

Other target tags can be repo-defined through `target_schema` in the repo manifest.

- versions like `v1.0.0` normalize to `1.0.0`
- platform and OS aliases normalize into the canonical `platform` field
- architecture aliases normalize into the canonical `architecture` field

Current supported platform and OS aliases:

- `windows`, `win`, `win32`, `win64` -> `windows`
- `linux` -> `linux`
- `mac`, `macos`, `osx`, `darwin` -> `macos`
- `any` -> `any`

Current supported architecture aliases:

- `x86_64`, `x64`, `amd64`, `win64` -> `x86_64`
- `x86_32`, `x86`, `win32` -> `x86_32`
- `any` -> `any`

For `repo init` and `target insert`, Gutch also tries to infer missing tag values from the input directory or file name. Built-in inference exists for:

- `version`
- `platform`
- `arch`

Example:

```text
my-game-v1.0.0-win-x64
```

Normalizes to:

- `version=1.0.0`
- `platform=windows`
- `arch=x86_64`

## 1. Repo Create

`repo create` creates an empty local content repo with `repo.json`, `targets/`, and `bundles/`.

`--product` is optional for local-only repos. A repo created without a product binding can still be managed locally, but remote publish or sync flows must bind to a backend product before content can be sent upstream.

### Example

```text
gutch repo create \
  --product my-game \
  --content-repo base \
  --repo ./.content_repo
```

Local-only repo without a product binding:

```text
gutch repo create \
  --content-repo sandbox \
  --repo ./.content_repo
```

Optional schema file:

```text
gutch repo create \
  --product my-game \
  --content-repo base \
  --schema ./target-schema.json \
  --repo ./.content_repo
```

## 2. Repo Discover Source Targets

`repo discover-source-targets` scans one source targets root and lists only immediate subfolders whose names match the content repo target schema.

Use it when a GUI or automation flow needs to treat each matching subfolder as a truth target install candidate for `target insert`.

Optional reporting output:

- `--json-out <path>` writes machine-readable candidate target install JSON.

### Example

```text
gutch repo discover-source-targets \
  --repo ./.content_repo \
  --source-targets-root ./truth-targets \
  --json-out ./.artifacts/discovered-source-targets.json
```

## 3. Repo Inspect

`repo inspect` reports local repo status for automation or GUI callers without reimplementing repo-manifest, target-manifest, and bundle-inspection rules outside `gutch`.

The command prints a human summary to stdout.

Optional reporting output:

- `--json-out <path>` writes machine-readable JSON with repo status, targets, bundle totals, and missing-bundle counts.

### Example

```text
gutch repo inspect \
  --repo ./.content_repo \
  --json-out ./.artifacts/repo-inspect.json
```

## 4. Repo Schema Set

`repo schema-set` replaces or clears the repo-defined target schema in `repo.json`.

### Example

```text
gutch repo schema-set \
  --repo ./.content_repo \
  --schema ./target-schema.json
```

Clear the custom schema and fall back to default identity behavior:

```text
gutch repo schema-set \
  --repo ./.content_repo \
  --clear
```

## 5. Repo Init

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

`repo init` accepts canonical tag keys and these input aliases:

- `os` as an alias for `platform`
- `architecture` as an alias for `arch`

Explicit spec tags win over folder-name inference.

`repo init` also accepts an optional `target_schema` in the spec file.

Example:

```json
{
  "target_schema": {
    "target_tags": [
      {
        "name": "loader",
        "required": true,
        "values": ["fabric", "forge", "neoforge"]
      },
      {
        "name": "mc",
        "required": false,
        "values": ["1.20.1", "1.21"]
      }
    ],
    "target_id_order": ["version", "loader", "mc"]
  },
  "targets": [
    {
      "dir": "./builds/ae2-v15.4.10-neoforge-1.21",
      "tags": {
        "version": "15.4.10"
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

After writing the repo, `repo init` also prints a storage summary that includes:

- total logical install size across all selected targets
- final on-disk repo size
- repo size as a percentage of the full combined install size
- saved percentage versus storing full copies of every target
- per-target added repo bytes in target order

## 6. Target Insert

`target insert` creates one target from one source directory and inserts it into an existing content repo.

It reads product and repo identity from the repo root's `repo.json`.
The source directory defaults to the current working directory.
For custom repo-defined target categories, use repeated `--tag name=value` arguments.
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

Or let Gutch infer the missing identity values from the input name:

```text
gutch target insert \
  --repo ./.content_repo \
  --dir ./builds/my-game-v1.0.0-win-x64
```

If a value is provided explicitly, the explicit CLI value wins over the folder-name inference.

Example with custom repo-defined tags:

```text
gutch target insert \
  --repo ./.content_repo \
  --dir ./builds/ae2-v15.4.10-neoforge-1.21 \
  --version 15.4.10 \
  --tag loader=neoforge \
  --tag mc=1.21
```

## 7. Target Export

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

## 8. Target Estimate

### Example

```text
gutch target estimate \
  --dir ./builds/base \
  --product my-game \
  --version 1.0.0
```

## 9. Target Install Fetch

`target_install fetch` prepares a local content repo for one target by downloading `manifest.json`, optional `manifest.sig`, and any missing required bundles.

Use it when the install flow has backend-authorized URLs for target content and you want `gutch` to own the local repo materialization instead of reimplementing that behavior in another client.

If `--bundle-urls-endpoint` is provided and explicit `--bundle <bundle-id>=<url>` values are not, Gutch requests bundle URLs for the target's required bundle IDs and then downloads any missing bundles.

### Example

```text
gutch target_install fetch \
  --repo ./.content_repo \
  --target 1.1.0-windows-x86_64-base \
  --manifest-url https://example.com/manifest.json \
  --manifest-sig-url https://example.com/manifest.sig \
  --bundle-urls-endpoint https://example.com/bundles \
  --session-id desktop-session-token
```

## 10. Target Install Apply

`target_install apply` reconstructs a selected target into a staging directory, verifies it, and then promotes it to the active install directory.

When `--source-target` is provided, Gutch verifies the existing source install first and reuses matching chunks from it before reading required bundle chunks. If `--source-dir` is omitted, the active install directory passed through `--dir` is used as the source install.

### Example

```text
gutch target_install apply \
  --repo ./.content_repo \
  --target 1.1.0-windows-x86_64-base \
  --dir ./install/base \
  --staging-dir ./install/.temp/1.1.0-windows-x86_64-base \
  --source-target 1.0.0-windows-x86_64-base \
  --summary-out ./.artifacts/target-install-apply.summary.txt \
  --stats-json-out ./.artifacts/target-install-apply.stats.json
```

`target_install apply` expects required bundles to already be available in the local content repo. Use `target_install fetch` first when the repo still needs manifest or bundle content downloaded.

## 11. Target Install Verify

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

## 12. Bundle Inspect

### Example

```text
gutch bundle inspect --bundle ./.content_repo/bundles/abc123.bundle
```

## 13. Path Compare

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
