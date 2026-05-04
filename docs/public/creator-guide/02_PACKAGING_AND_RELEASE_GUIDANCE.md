# Packaging And Release Guidance

## What Good Inputs Look Like

A product usually behaves well in Gutch when it has:

- stable file names
- stable archive names
- stable file/level/scene grouping
- stable metadata where possible
- optional content separated from core content
- builds that do not rewrite unrelated files every release

This is true for games, apps, and asset packs alike.

For example, these are usually good signs:

- a game keeps maps in separate archives instead of one giant monolith
- an app keeps language resources in separate bundles
- an image pack publishes themed collections as separate archives instead of rebuilding one giant everything file

## Separate Content By Ownership And Purpose

The cleanest Gutch setups separate content by ownership and purpose.

Base content should be distinct from optional add-ons. Language packs should be distinct from core assets when possible. User-owned content such as mods, saves, logs, screenshots, caches, and config should not be mixed into repair-owned install content unless there is no practical alternative.

A reasonable default layout looks like this:

```text
product/
  core/
  addons/
  lang/
  mods/
  saves/
  logs/
```

Not every runtime can keep such a clean physical layout, especially in older engines or tightly constrained packaging systems. Even then, the ownership model should stay clear in your release process.

## Core Content

If your product has a core install, that content should usually be the most stable stream you publish.

Prefer:

- stable archive names
- stable asset grouping
- files grouped by feature or content domain
- reproducible packaging settings

Avoid:

- one giant archive rebuilt every release for unrelated changes
- random asset ordering
- fresh embedded timestamps unless they are required
- build output that rewrites everything for small changes

## Release Process Discipline

Most of the quality of a Gutch integration comes from release discipline, not from one clever trick.

Prefer:

- reproducible builds
- stable packaging rules
- small targeted changes
- content grouped by feature, module, asset family, language, region, or add-on boundary

Avoid:

- changing compression settings casually every release
- moving files between archives without a good reason
- repacking all content because one small file changed

## Packaging Patterns That Usually Work Well

These patterns tend to work well:

```text
base_assets.pak
characters.pak
maps_forest.pak
maps_desert.pak
audio_en.pak
audio_tr.pak
dlc_expansion_01.pak
dlc_soundtrack_01.pak
```

Outside games, the same idea looks like this:

```text
app_core.zip
plugin_sdk.zip
docs_en.zip
docs_tr.zip
brush_pack_basic.zip
brush_pack_pro.zip
textures_starter.zip
textures_extended.zip
```

These layouts usually improve:

- reuse
- upload size
- optional-content ownership
- repair scope
- long-term retention of versions

## Packaging Patterns That Usually Work Poorly

These patterns usually work poorly:

```text
everything.pak
everything_with_all_languages_and_dlc.pak
release_build_with_random_ordering_and_embedded_timestamps.pak
```

Outside games, the same anti-patterns look like this:

```text
app_everything.zip
all_assets_and_all_languages_in_one_archive.zip
full_rebuild_output_with_randomized_file_order.zip
```

These layouts tend to cause broad repacking, weaker reuse, larger uploads, and blurrier ownership boundaries.
