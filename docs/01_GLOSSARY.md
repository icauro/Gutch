# Gutch Glossary

Status: Directional, ready for first implementation pass  
Owner: Artifact Platform  
Last reviewed: 2026-05-02

## Purpose

This document defines the canonical terms used across Gutch docs and code discussions.

## Core Terms

### Product

The top-level catalog and commercial entity.

A product owns:

- marketing and store identity
- entitlement rules
- public-facing product information
- one or more content repos

### Content Repo

A remote Gutch-managed storage container for one product content stream.

A content repo is the holding storage in R2/S3 that receives targets and bundles produced by Gutch build work on a creator machine.

A content repo owns one or more targets.

Examples:

- base content repo
- DLC content repo
- language-pack content repo

### Target

One installable build identity owned by a content repo.

A target is an installable recipe produced from content in one content repo.

Current compact model:

```text
target = content repo + version + platform + architecture + tier
```

Example:

```text
base_repo:v2.0.0:windows:x64:premium
```

### Target Tags

Target tags describe the variant axes for one target.

Current canonical required tags are:

- `version`
- `platform`
- `arch`

Current default values:

- `platform=any`
- `arch=any`

`any` means the target is intentionally valid across that axis. It does not mean the value is unknown.

Common optional tags include:

- `tier`
- `edition`
- `language`
- `channel`

### Target Manifest

The authoritative signed description of one target.

The target manifest tells the launcher what to install and verify.

### File Manifest

A file-manifest entry describes one logical file, including its metadata and chunk layout.

In the current Gutch design, file manifests are embedded inside the target manifest instead of being stored as separate JSON objects.

### Artifact

Any stored delivery object used by Gutch.

Examples:

- target manifest
- file manifest
- bundle

### Bundle

A shared artifact that stores many compressed chunks together.

Bundles are the physical object-store unit. Chunks are the logical deduplication unit.

Bundles are not target-exclusive. Multiple targets can reference the same bundle.

Published bundles are immutable. If a later target introduces new chunks, Gutch creates new bundle objects for those chunks instead of appending to or rewriting existing bundles.

### Chunk

A content-defined unit of file data.

Chunks are identified by BLAKE3 over raw uncompressed bytes.

Chunks are immutable. If the raw bytes change, the BLAKE3 hash changes and the result is a different chunk.

### Install

The realized local result for a target.

Use these more specific terms when needed:

- `staged install`: reconstructed locally but not yet promoted
- `active install`: the currently live local install

## Relationship Model

```text
product
-> content repos
-> targets
-> target manifest
-> embedded file manifests
-> chunk references
-> bundles
-> install
```

Expanded hierarchy:

```text
product
├─ marketing/store identity
├─ entitlement rules
├─ public-facing info
├─ content repos
│  ├─ base content repo
│  │  ├─ source snapshots
│  │  └─ targets
│  │     ├─ target: version/platform/arch/tier
│  │     └─ target: version/platform/arch/tier
│  ├─ dlc content repo
│  │  ├─ source snapshots
│  │  └─ targets
│  │     └─ target: version/platform/arch/tier
│  └─ language-pack content repo
│     ├─ source snapshots
│     └─ targets
│        └─ target: version/platform/arch/tier
```

## Naming Guidance

- Use `product` for the catalog and commercial container.
- Use `content repo` for the remote content-lineage and storage container.
- Use `target` for identity.
- Use `target manifest` for the signed install contract.
- Use `artifact` for stored delivery objects in general.
- Use `bundle` for shared chunk-container artifacts.
- Use `install` for local realized state.

## Binary Storage Units

Use IEC binary units for storage sizes in Gutch docs and manifests:

- `KiB` = 1024 bytes
- `MiB` = 1024 KiB
- `GiB` = 1024 MiB

Use these names when the size is based on binary powers of two, such as chunk and bundle sizing. Use `bytes` only for raw byte counts, and avoid `KB` / `MB` / `GB` unless you explicitly mean decimal units.

Examples:

- `64 KiB` chunk average
- `16 MiB` preferred bundle size
- `128 MiB` maximum bundle size

## Notes

- `version` alone is not enough to identify a target once platform, architecture, and tier differ.
- Do not shorten `content repo` to plain `repo` in contexts where it could be confused with a Git repository.
- `artifact` alone is too broad to replace `target`.
- `bundle` should stay distinct from `target artifact` because bundles are commonly shared across targets.
