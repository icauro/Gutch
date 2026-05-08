# Gutch Architecture

Status: Canonical
Owner: Artifact Platform
Last reviewed: 2026-05-08

## Purpose

Describe Gutch code placement and engine boundaries. This file is about structure, not task routing or validation.

## Layers

- `src/main.rs` owns CLI entrypoint behavior.
- `src/lib.rs` exposes reusable engine capabilities.
- `src/` owns chunking, bundle handling, manifest logic, reconstruction, and verification internals.
- `examples/` owns runnable fixture and comparison workflows for engine validation.
- `docs/` owns the canonical design and CLI contract.

## Dependency Direction

Allowed direction is:

```text
src/main.rs -> src/lib.rs -> src/*
examples -> CLI or library behavior
docs -> no code dependency
```

CLI parsing should stay at the entrypoint; engine behavior should live in reusable library code.

## Rules

- Keep terminology aligned with `docs/01_GLOSSARY.md`.
- Trust-boundary behavior such as signed manifests and verification must stay explicit and testable.
- CLI examples should track real command behavior instead of becoming aspirational docs.
- Prefer simple, inspectable storage and reconstruction flows over hidden magic.
- Keep fixture and comparison workflows outside core engine modules.

## Avoid

- Hiding manifest semantics inside CLI-only code paths.
- Duplicating verification rules across reconstruct and verify flows.
- Letting docs invent behavior that the library does not expose.
