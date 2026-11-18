# Gutch CLI Builds

Status: Current

Scope: Release builds of `gutch-cli` in GitHub Actions

`.github/workflows/build-cli.yml` builds `gutch-cli` in release mode with the
locked Cargo dependencies and Rust 1.100.0. It runs manually, when relevant
code/build files change on `main`, and as part of a [release](07_releases.md).
Before building, each runner writes the license notices for its target with
`_tools/package_cli.py notices`: Gutch's own licenses and adapted-source
notices, the license files of every Cargo dependency linked into the CLI for
that target (identical texts printed once, with every package that uses them),
and the Rust standard library's notices. A dependency without notices fails
this step. `GUTCH_CLI_NOTICES` points the build at that file, and
`gutch-cli/build.rs` embeds it compressed, so `gutch-cli licenses` prints it.
Builds without the variable, including `cargo install gutch-cli`, embed only
the CLI's own license and a note that the rest live in each dependency's
source.

Each native runner then executes `--help` and `licenses`, failing if the
notices lack the standard library or known dependencies; in a release it also
checks that `--version` reports the version being released. The independent
correctness workflow remains the owner of the full test suites.

| Artifact | Rust target | Native runner |
|---|---|---|
| `gutch-cli-linux-x86_64` | `x86_64-unknown-linux-gnu` | Ubuntu 24.04 x86-64 |
| `gutch-cli-linux-arm64` | `aarch64-unknown-linux-gnu` | Ubuntu 24.04 ARM64 |
| `gutch-cli-windows-x86_64` | `x86_64-pc-windows-msvc` | Windows Server 2025 x86-64 |
| `gutch-cli-windows-arm64` | `aarch64-pc-windows-msvc` | Windows 11 ARM64 |

Download artifacts from a successful **Build Gutch CLI** run. Each package
contains the executable, `SHA256SUMS`, and `build.json` with the commit,
compiler, target, checksum and linked dependency list.
These are unsigned CI builds retained for 30 days. Linux builds use GNU libc
from Ubuntu 24.04; runtime system libraries are not bundled.

`_tools/package_cli.py package` stages packages under `.builds/releases/<target>/`
and `notices` writes to `.builds/notices/<target>.txt`.
Intermediate compilation and dependency caches stay under
`.builds/compiler-cache/cargo/`; neither binaries nor generated notices enter Git.
