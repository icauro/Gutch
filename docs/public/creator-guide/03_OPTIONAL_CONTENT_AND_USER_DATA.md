# Optional Content And User Data

## Optional Add-Ons

If your product has optional add-ons, treat them as first-class installable content.

That applies to DLC in games, plugin packs in software, premium resource sets in apps, and expansion packs in asset libraries.

Prefer:

- separate content streams for meaningful add-ons
- separate installable targets for those add-ons
- additive content where possible
- clear ownership boundaries between core content and optional content

Avoid:

- hiding add-ons inside the main content stream with no clear boundary
- packaging add-ons so they silently replace unrelated core files unless the runtime genuinely requires it

## Language Packs

Language content should also be additive whenever possible.

If only one language changed, that should not force the whole product to look new.

Prefer:

- separate language packs
- separate archives for language-specific assets
- clean language-specific file groups such as `audio_en`, `audio_tr`, subtitle packs, localized docs, or localized UI resources

Avoid:

- rebuilding the full core content set for one language change
- mixing every language into one large archive when the packaging can be split safely

## Mods, Saves, Logs, And User Data

Repair should not destroy user content by default.

That means creators should keep user-owned content out of repair-owned install paths whenever possible.

Prefer:

- dedicated mod directories
- dedicated save directories
- dedicated config and log directories
- keeping screenshots and caches outside core install content

Avoid:

- mixing saves, logs, screenshots, and caches into core install content
- requiring mods to replace arbitrary core files if a cleaner extension model is possible
