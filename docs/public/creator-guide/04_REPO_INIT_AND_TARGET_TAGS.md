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

Current required target tags are:

- `version`
- `platform`
- `arch`

For shared content, `platform=any` and `arch=any` are valid explicit values. They mean the content is intentionally not restricted on that axis. They do not mean the value is unknown.

Common optional tags include:

- `tier`
- `edition`
- `language`
- `channel`

Keep tag usage predictable. If the same concept is represented by different tag conventions over time, the repo becomes harder to reason about.
