# First Repo Init And Target Tags

## First Repo Initialization

When onboarding a product, decide these things early:

- what your content repos should be
- which targets belong together in the initial publish
- whether first generation should behave like one batch pack or a sequence of target commits
- what target tags you expect to stay stable over time

In general:

- use batch init when you want the best early bundle fill and cross-target reuse inside a new content repo
- use sequential init when you want initialization to behave like repeated one-target-at-a-time commits

## Target Tags

`version` is always required.

All other target tags are repo-defined through `target_schema` in `repo.json`.

Gutch normalizes built-in tags into canonical manifest values.

- `version`: `v1.0.0` becomes `1.0.0`
- `platform`: OS aliases normalize into the canonical platform value
- `arch`: architecture aliases normalize into the canonical architecture value

Supported platform and OS aliases:

- `windows`, `win`, `win32`, `win64` -> `windows`
- `linux` -> `linux`
- `mac`, `macos`, `osx`, `darwin` -> `macos`
- `any` -> `any`

Supported architecture aliases:

- `x86_64`, `x64`, `amd64`, `win64` -> `x86_64`
- `x86_32`, `x86`, `win32` -> `x86_32`
- `any` -> `any`

When reading init specs, Gutch also accepts these aliases for the built-in tags:

- `os` as an alias for `platform`
- `architecture` as an alias for `arch`

For shared content, `platform=any` and `arch=any` are valid explicit values. They mean the content is intentionally not restricted on that axis. They do not mean the value is unknown.

Common repo-defined tags include:

- `tier`
- `edition`
- `language`
- `channel`

Keep tag usage predictable. If the same concept is represented by different tag conventions over time, the repo becomes harder to reason about.

If a built-in tag is omitted, Gutch may infer `version`, `platform`, and `arch` from the input build folder or file name when it recognizes a supported pattern such as `my-game-v1.0.0-win-x64`.

Example schema:

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
  }
}
```

That repo would require `version` and `loader`, optionally allow `mc`, and generate target IDs like `15.4.10-neoforge-1.21`.
