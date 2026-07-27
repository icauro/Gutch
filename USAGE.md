# Gutch CLI Usage

Canonical documentation lives in [`docs/blueprint/gutch/`](../../docs/blueprint/gutch/README.md):

| Doc | Topic |
|---|---|
| [`README.md`](../../docs/blueprint/gutch/README.md) | Index, constants, engine surface |
| [`12_CLI_USAGE.md`](../../docs/blueprint/gutch/12_CLI_USAGE.md) | Command reference |
| [`09_TARGET_TAGS.md`](../../docs/blueprint/gutch/09_TARGET_TAGS.md) | Tag schema, inference, target ID rules |
| [`06_DELIVERY_FLOWS.md`](../../docs/blueprint/gutch/06_DELIVERY_FLOWS.md) | Build, publish, install, repair |

## Quick start

All filesystem arguments are required and absolute. The examples use Linux
paths; Windows callers use absolute drive paths such as `D:\Repos\my-repo`.

```sh
pl-cli gutch repo init --path /home/patron/repos/my-repo
pl-cli gutch target insert --dir /home/patron/builds/1.0.0-windows-x86_64-base --repo /home/patron/repos/my-repo
pl-cli gutch repo link --repo /home/patron/repos/my-repo --product-id 42
pl-cli gutch repo sync-up --repo /home/patron/repos/my-repo --yes
```

Automation and CI: pass `--yes` on `target insert`, `insert-many`, and `target-sources sync`. To preview a plan without applying it, run the command and answer `n` at the prompt.

## Launch options

```sh
pl-cli gutch repo launch-set --repo /home/patron/repos/my-repo --launch /home/patron/repo-launch.json
pl-cli gutch repo launch-set --repo /home/patron/repos/my-repo --clear
pl-cli gutch target launch-set --repo /home/patron/repos/my-repo --target 1.0.0-windows-x86_64 --launch /home/patron/target-launch.json
pl-cli gutch target launch-set --repo /home/patron/repos/my-repo --target 1.0.0-windows-x86_64 --clear
```

The JSON formats and inheritance behavior are defined in
[`21_LAUNCH_OPTIONS.md`](../../docs/blueprint/gutch/21_LAUNCH_OPTIONS.md).
