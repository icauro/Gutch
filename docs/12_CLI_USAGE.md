# Gutch CLI Usage

For canonical terminology and core design rules, start with `01_GLOSSARY.md`, `03_TARGETS_AND_MANIFESTS.md`, and `04_DELIVERY_FLOWS.md`.

## Purpose

This document is the canonical usage guide for the `gutch` CLI.

Keep command-line usage, examples, and spec-file formats here instead of scattering them through design docs.

## Current Commands

- `gutch repo-init`
- `gutch build`
- `gutch reconstruct`
- `gutch verify`
- `gutch bundle-info`
- `gutch estimate`

## 1. Repo Init

`repo-init` creates a new content repo and populates it with an initial batch of targets.

It is batch-oriented by design:

- each target is scanned and chunked first
- chunk reuse is computed across the full batch
- bundling happens after the full batch has already been chunked

### Example

```text
gutch repo-init \
  --product my-game \
  --content-repo base \
  --init-mode batch \
  --spec init-spec.json \
  --output ./artifacts
```

### Init Modes

`repo-init` supports two initialization modes:

- `--init-mode batch`
  Chunk every target first, then dedupe and bundle across the full init batch.
- `--init-mode sequential`
  Treat init as a series of one-target-at-a-time commits in spec order.

Use `batch` when you want the best early bundle fill and cross-target reuse inside the new content repo.

Use `sequential` when you want first-time repo creation to behave like repeated commits, preserving commit order effects on reuse and bundle packing.

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

### Tag Rules

Current tag behavior:

- `version` is required
- `platform` defaults to `any` when omitted
- `arch` defaults to `any` when omitted
- `tier` defaults to `base` when omitted

`any` means the target is intentionally valid across that axis.

### Output Layout

`repo-init` writes into:

```text
v1/products/{product_id}/repos/{content_repo_id}/
  targets/{target_id}/manifest.json
  bundles/{bundle_id}.bundle
  metadata/
```

## 2. Build

`build` creates one target from one source directory.

Use this for the current single-target build path.

### Example

```text
gutch build \
  --dir ./builds/base \
  --product my-game \
  --content-repo base \
  --version 1.0.0 \
  --platform windows \
  --architecture x86_64 \
  --tier base \
  --output ./artifacts
```

`build` writes the target manifest into the selected content repo and reuses bundles already present in that repo when possible.

## 3. Reconstruct

### Example

```text
gutch reconstruct \
  --manifest ./artifacts/v1/products/my-game/repos/base/targets/1.0.0-windows-x86_64-base/manifest.json \
  --bundles ./artifacts/v1/products/my-game/repos/base/bundles \
  --output ./reconstructed
```

## 4. Verify

### Example

```text
gutch verify \
  --manifest ./artifacts/v1/products/my-game/repos/base/targets/1.0.0-windows-x86_64-base/manifest.json \
  --dir ./install
```

`verify` currently reports:

- missing files
- corrupt files
- extra files

## 5. Bundle Info

### Example

```text
gutch bundle-info --bundle ./artifacts/v1/products/my-game/repos/base/bundles/abc123.bundle
```

## 6. Estimate

### Example

```text
gutch estimate \
  --dir ./builds/base \
  --product my-game \
  --version 1.0.0
```

## Notes

- `repo-init` is the preferred entrypoint for first generation of a new content repo when multiple targets should be packed together.
- `repo-init --init-mode sequential` is available when first generation should behave like repeated single-target commits instead of one batch pack pass.
- `build` remains a single-target path.
- Manifest signing is still planned but not fully implemented in the CLI yet.
