# Gutch Integrity And Repair

For canonical terminology and core design rules, start with `01_GLOSSARY.md`, `03_TARGETS_AND_MANIFESTS.md`, and `04_DELIVERY_FLOWS.md`.

## Purpose

This document explains how Gutch checks install integrity and how repair should work when files, chunks, or install metadata are missing or corrupted.

Current status:

- Full repair planning and execution are design targets for Gutch, not fully implemented behavior yet.
- The current library implementation can verify manifest-declared files and classify install-state issues as missing, corrupt, or extra.
- Signed target manifests are part of the intended trust model, but signature enforcement is not fully wired into the current implementation yet.

## Integrity Model

Gutch verifies integrity at multiple layers:

1. `target manifest`
2. `chunk`
3. `file`
4. `install state`

The system should never trust a local install only because files happen to exist on disk.

## 1. Target Manifest Integrity

The target manifest is the trust root for one target.

Rules:

- The target manifest must be signed.
- Clients must reject unsigned target manifests.
- Clients must reject target manifests with invalid signatures.
- Clients must not guess trust-sensitive storage paths.

If the target manifest is not trusted, repair must stop before any reconstruction work begins.

## 2. Chunk Integrity

Each chunk is identified by BLAKE3 over the raw uncompressed bytes.

Rules:

- Chunk identity must not be derived from compressed bytes.
- Downloaded chunks must be validated before use.
- Reused local chunks must also be validated before use if full repair is running.
- A chunk that fails validation is treated as missing.

This lets Gutch reuse valid local data while rejecting corrupt data at chunk granularity.

## 3. File Integrity

Each embedded file-manifest entry records the full-file hash and chunk layout.

Rules:

- Reconstructed files must match the expected file size.
- Reconstructed files must match the expected full-file hash.
- A file is not trusted until full-file verification succeeds.

Chunk validation protects the pieces. Full-file validation protects the final assembled result.

## 4. Install State Integrity

The local install database is a cache of what the launcher believes is installed.

It tracks install metadata and staged-state transitions. It should not be used as a durable local chunk payload cache by default.

Rules:

- The database must be rebuildable from manifests and disk state.
- The database must not be the only source of truth.
- Quick verification may trust cached metadata.
- Full repair must be able to recover even if the install database is stale or partially wrong.

## Quick Verify

Quick verify is the cheap routine check.

This section describes the intended lightweight launcher-oriented check, not the current `gutch` library verification routine.

It should verify:

```text
file exists
size matches
mtime matches if trusted
mode matches
install.db is internally consistent
```

Use quick verify when:

- starting the launcher
- checking whether a target probably still matches local state
- deciding whether a full repair is necessary

Quick verify is intentionally fast. It does not prove that every byte is correct.

Current implementation note:

- The current `gutch` verifier is stricter than this design target.
- It verifies manifest-declared files by reading file contents, hashing the full file, and hashing each declared chunk span.
- It also reports extra on-disk files that are not present in the target manifest.

## Full Repair

Full repair is the explicit recovery flow.

This section describes the intended recovery flow. The current codebase does not yet expose a complete repair planner/executor that performs all of these steps end-to-end.

It should run this process:

```text
scan install directory
hash files or chunks
compare against target manifest
identify missing/corrupt chunks
reuse valid local chunks
download only missing/corrupt data
reconstruct affected files in staging
verify before replacement
```

Use full repair when:

- quick verify fails
- the user requests repair
- the install DB is missing or unreliable
- files were manually modified
- corruption is suspected after crash or interrupted update

## Repair Planning

Repair should reuse as much valid local data as possible.

Planner inputs:

- target manifest
- embedded file-manifest entries
- local install state
- local file scan results
- required bundle set from the target manifest
- bundle chunk tables

Planner outputs:

- valid local chunks to keep
- missing chunks to fetch
- affected files to rebuild
- bundle or range requests needed for recovery

## Repair Execution

Repair should not modify the active install in place by default.

Preferred execution model:

1. validate target manifest
2. scan and classify local files
3. validate reusable local chunks
4. download missing or corrupt chunks
5. reconstruct affected files into staging
6. verify reconstructed files
7. replace or promote only after verification succeeds
8. rebuild or refresh install DB metadata

This keeps recovery safe even if power is lost or the process crashes during repair.

## What Happens When Corruption Is Found

If corruption is detected:

- corrupt chunks are treated as unavailable
- missing chunks are fetched from bundles
- only affected files need reconstruction
- unaffected verified files can stay in place

If extra files are detected:

- files present on disk but absent from the target manifest should be classified separately from missing or corrupt manifest-owned files
- extra files should not be deleted automatically by default
- repair policy may later choose to warn, quarantine, or explicitly remove extras depending on launcher mode and product policy

Gutch should avoid broad recovery work unless the planner decides that grouped bundle downloads are better than highly fragmented range requests.

## Fragmented Recovery

If chunk-based recovery is too fragmented or too expensive, the planner may choose coarser-grained bundle downloads instead of many tiny range requests.

Examples:

- too many bundles are touched
- too many tiny range reads are needed
- local state is heavily damaged
- grouped bundle downloads are operationally simpler or faster

## Safety Rules

- Never trust unsigned target manifests.
- Never trust chunk bytes that fail hash validation.
- Never trust reconstructed files before full-file verification.
- Never modify the active install in place before staged validation succeeds.
- Treat install DB metadata as helpful cache, not absolute truth.

## Short Mental Model

```text
verify trust root
-> verify reusable local data
-> download only missing/corrupt data
-> rebuild in staging
-> verify final files
-> promote safe result
```
