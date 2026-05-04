# Gutch Mods, DLCs, And Language Packs

For canonical terminology and core design rules, start with `01_GLOSSARY.md`, `03_TARGETS_AND_MANIFESTS.md`, `04_DELIVERY_FLOWS.md`, and `09_INTEGRITY_AND_REPAIR.md`.

## Purpose

This document defines how Gutch should reason about optional content and user-added content during install, verify, repair, and uninstall flows.

The core goal is to avoid treating all non-base files the same.

Gutch should distinguish between:

- manifest-owned base content
- manifest-owned optional content such as DLCs or language packs
- user-owned or third-party-owned content such as mods
- unknown files on disk

## Core Rule

Repair and verification should operate on content ownership, not only on whether a file exists on disk.

This means:

- a missing manifest-owned file is a repair target
- a corrupt manifest-owned file is a repair target
- a user-owned file is not a repair target
- an unknown extra file should be classified explicitly instead of being treated as automatic corruption

## Content Categories

### Base Content

Base content is the core installed target selected by:

```text
content_repo_id + version_id + platform + architecture + tier_id
```

Rules:

- Base content is manifest-owned.
- Base content is verified strictly against the active manifest set.
- Base content may be repaired automatically when missing or corrupt.

### DLCs

DLCs are optional owned add-ons that should behave like first-class installable content, not like loose extras.

Preferred model:

- represent each DLC as its own installable target or explicit overlay
- track whether the DLC is installed and enabled
- include enabled DLC manifests in the active manifest set for verify and repair

Rules:

- Installed DLC files are manifest-owned.
- Uninstalled DLC files should not silently remain in manifest-owned paths unless policy allows it.
- DLCs should be installable, repairable, and removable independently from the base target when possible.

### Language Packs

Language packs are optional owned content similar to DLCs, but they should be designed to remain additive whenever possible.

Preferred model:

- use separate manifests or overlays for each language pack
- keep language assets in distinct paths or archives when packaging allows it
- activate one or more language packs by adding their manifests to the active manifest set

Rules:

- Installed language pack files are manifest-owned.
- Language packs should avoid replacing unrelated base files when possible.
- Verification should treat enabled language-pack manifests the same way it treats base content manifests.

### Mods

Mods are usually user-managed or third-party-managed content.

By default, mods should not be treated as corruption.

Rules:

- Mods should live outside strict manifest-owned roots when possible.
- Mods should not be deleted automatically during repair by default.
- Mods should be classified as user-owned or external-owned content, not as normal manifest drift.
- If a mod replaces a manifest-owned file, that collision should be surfaced clearly.

## Directory Layout Guidance

The safest strategy is to avoid mixing ownership domains in the same directory tree when packaging or launcher policy can control it.

Prefer layouts such as:

```text
game/
  base/
  dlc/
  lang/
  mods/
  saves/
  logs/
```

If the actual game engine requires a flatter structure, Gutch should still maintain a logical ownership model even when physical paths are mixed.

## Active Manifest Set

Verification and repair should compare disk state against the active manifest set, not only one base manifest.

The active manifest set is the union of:

- the selected base target manifest
- enabled DLC manifests
- enabled language-pack manifests

This lets Gutch answer:

- which files are expected
- which files are missing
- which files are corrupt
- which files are extra relative to currently enabled owned content

## Path Ownership Model

Gutch should eventually classify paths by ownership.

Suggested path ownership classes:

```text
base-owned
addon-owned
user-owned
launcher-owned
unknown
```

Interpretation:

- `base-owned` means owned by the selected base target manifest
- `addon-owned` means owned by an enabled DLC or language-pack manifest
- `user-owned` means mod, save, screenshot, config, or other user-controlled content
- `launcher-owned` means app-created support files such as caches, temp files, or local metadata
- `unknown` means present on disk but not recognized by current policy

Repair behavior should depend on the ownership class.

## Verification Policy

### Owned Content

For `base-owned` and `addon-owned` files:

- verify existence
- verify size
- verify full-file hash when full verification is running
- verify chunk hashes when full verification is running
- classify missing or corrupted files as repair targets

### User-Owned Content

For `user-owned` files:

- do not treat presence as corruption
- do not delete automatically during repair by default
- optionally warn if a file conflicts with a manifest-owned path

### Unknown Content

For `unknown` files:

- classify them explicitly as extras
- do not auto-delete by default
- allow launcher policy to decide whether to ignore, warn, quarantine, or remove them

## Collision Handling

The most important edge case is a file-path collision between manifest-owned content and modded or unknown content.

Examples:

- a mod overwrites a base `.pak`
- a language pack drops files into the same path as the base install
- a stale DLC file remains after the DLC was disabled

Recommended behavior:

- detect the collision
- classify the file as conflicting
- explain that restoring official content may overwrite local changes
- prefer quarantine or explicit confirmation over silent deletion

## Repair Modes

Different repair intents should use different policies.

### Safe Repair

Safe repair should be the default user-facing mode.

Rules:

- repair missing or corrupt manifest-owned files
- leave extras alone by default
- warn about conflicting files
- avoid deleting mods, saves, screenshots, logs, or unknown extras

### Strict Repair

Strict repair restores the install to the exact active manifest set.

Rules:

- repair missing or corrupt manifest-owned files
- quarantine or remove conflicting extras inside manifest-owned roots
- leave clearly user-owned roots alone unless the user explicitly chose cleanup

### Clean Reinstall

Clean reinstall is stronger than repair.

Rules:

- wipe manifest-owned install roots
- rebuild from the selected active manifest set
- keep user-owned roots only when product policy says they are outside the owned install root

## Packaging Guidance

Optional and user-owned content becomes much easier to manage when the game packaging layout helps Gutch.

Prefer:

- additive DLC assets
- additive language packs
- separate archives per optional content group
- stable mod directories or stable mod-owned paths
- clear save/config/log directories outside repair-owned paths

Avoid when possible:

- one giant archive for base game plus all DLC plus all languages
- packaging that forces mods to replace arbitrary core files without any ownership boundaries
- mixing saves, logs, and caches into manifest-owned directories

## Practical Default Policy

For an early Gutch implementation, the safest default is:

- Base target manifest plus enabled add-on manifests define the owned file set.
- Missing or corrupt owned files are repair candidates.
- Extra files are reported separately.
- Extra files are not deleted automatically by default.
- Mods should live in dedicated paths whenever the product allows it.
- DLCs and language packs should be modeled as explicit manifests or overlays, not as undocumented extras.

## Open Follow-Ups

- Define how the launcher stores enabled DLC and language-pack state.
- Define whether overlays or full manifests are the preferred target model for optional content.
- Define how path ownership rules are configured per product.
- Define whether quarantine paths live beside the install or under launcher-managed storage.
- Define which launcher-owned files should be ignored automatically during verify.
