# Gutch CLI Builds

Status: Current

Scope: Release builds of `gutch-cli` in GitHub Actions

`.github/workflows/build-cli.yml` builds `gutch-cli` in release mode with the
locked Cargo dependencies and Rust 1.100.0. It runs manually, when relevant
code/build files change on `main`, and as part of a [release](07_releases.md).
Each native runner executes `--help` before packaging its executable; in a
release it also checks that `--version` reports the version being released. The
independent correctness workflow remains the owner of the full test suites.

| Artifact | Rust target | Native runner |
|---|---|---|
| `gutch-cli-linux-x86_64` | `x86_64-unknown-linux-gnu` | Ubuntu 24.04 x86-64 |
| `gutch-cli-linux-arm64` | `aarch64-unknown-linux-gnu` | Ubuntu 24.04 ARM64 |
| `gutch-cli-windows-x86_64` | `x86_64-pc-windows-msvc` | Windows Server 2025 x86-64 |
| `gutch-cli-windows-arm64` | `aarch64-pc-windows-msvc` | Windows 11 ARM64 |

Download artifacts from a successful **Build Gutch CLI** run. Each package
contains the executable, `SHA256SUMS`, commit/compiler/target provenance in
`build.json`, source notices, and target-specific Cargo dependency license files
indexed by `dependencies.json`, plus Rust standard-library notices. Missing
notices fail packaging.
These are unsigned CI builds retained for 30 days. Linux builds use GNU libc
from Ubuntu 24.04; runtime system libraries are not bundled.

`_tools/package_cli.py` stages packages under `.builds/releases/<target>/`.
Intermediate compilation and dependency caches stay under
`.builds/compiler-cache/cargo/`; neither binaries nor generated notices enter Git.
