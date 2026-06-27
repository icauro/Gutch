# Gutch CLI Usage

Canonical documentation lives in [`docs/blueprint/gutch/`](../../docs/blueprint/gutch/README.md):

| Doc | Topic |
|---|---|
| [`README.md`](../../docs/blueprint/gutch/README.md) | Index, constants, engine surface |
| [`12_CLI_USAGE.md`](../../docs/blueprint/gutch/12_CLI_USAGE.md) | Command reference |
| [`09_TARGET_TAGS.md`](../../docs/blueprint/gutch/09_TARGET_TAGS.md) | Tag schema, inference, target ID rules |
| [`06_DELIVERY_FLOWS.md`](../../docs/blueprint/gutch/06_DELIVERY_FLOWS.md) | Build, publish, install, repair |

## Quick start

```sh
plctl gutch repo init --path ./my-repo
plctl gutch target insert --dir ./builds/1.0.0-windows-x86_64-base --repo ./my-repo
plctl gutch repo link --repo ./my-repo --product-id 42
plctl gutch repo sync-up --repo ./my-repo --yes
```

Automation and CI: pass `--yes` on `target insert`, `insert-many`, and `target-sources sync`. To preview a plan without applying it, run the command and answer `n` at the prompt.
