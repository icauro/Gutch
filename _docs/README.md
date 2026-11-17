# Gutch user guide

Gutch stores builds of your software as snapshots in a repository and installs
any snapshot as an exact, verified copy of the original folder. Files and
snapshots share identical content, so a repository holding many versions stays
small, and updating an installation only writes what changed.

This guide is for people who use the `gutch-cli` program or embed the `gutch`
library in an application. It is read in order the first time; later, jump to
the page you need.

| Page | Covers |
| --- | --- |
| [1. Getting started](01_getting_started.md) | Get the program, store a first build, install it, check it, fix it |
| [2. Concepts](02_concepts.md) | Repositories, snapshots, tags, chunks, bundles and installations |
| [3. Capturing snapshots](03_capturing_snapshots.md) | Storing builds, tags, discovery, storage options, listing, comparing and deleting |
| [4. Installing snapshots](04_installing_snapshots.md) | Preview, reconstruct, verify and repair; reuse and downloads |
| [5. Managing repositories](05_managing_repositories.md) | Status, verification, filename and tag rules, index rebuild, remote sync |
| [6. Scripting and output](06_scripting_and_output.md) | JSON output, confirmations, exit codes, error reports, resources, profiling |
| [7. Troubleshooting](07_troubleshooting.md) | Common error messages and what to do about them |
| [8. Embedding Gutch](08_embedding_gutch.md) | Using the `gutch` Rust library and supplying remote providers |

## How do I…

| Goal | Command | Page |
| --- | --- | --- |
| Start a store for my builds | `repo init` | [1](01_getting_started.md#store-two-builds) |
| Store a build | `snapshot capture` | [3](03_capturing_snapshots.md#capture-a-folder) |
| Store a folder full of builds at once | `snapshot capture --discover-from` | [3](03_capturing_snapshots.md#capture-every-build-in-a-folder) |
| See what is stored | `snapshot list`, `repo status` | [3](03_capturing_snapshots.md#list-compare-and-inspect) |
| See what changed between two versions | `snapshot diff` | [3](03_capturing_snapshots.md#list-compare-and-inspect) |
| Install a version | `installation reconstruct` | [4](04_installing_snapshots.md#reconstruct) |
| Update an installation to a new version quickly | `reconstruct --reuse-dir` | [4](04_installing_snapshots.md#reuse-an-existing-installation) |
| Check or fix an installation | `installation verify`, `repair` | [4](04_installing_snapshots.md#verify) |
| Free space by removing a version | `snapshot delete` | [3](03_capturing_snapshots.md#delete-a-snapshot) |
| Protect a store with a password | `repo init --encrypt` | [5](05_managing_repositories.md#encryption) |
| Check that a store is healthy | `repo verify` | [5](05_managing_repositories.md#check-stored-content) |
| Use Gutch from a script or CI | `--json`, `--yes` | [6](06_scripting_and_output.md) |

## Reading the examples

The examples work in a Linux or macOS terminal and in Windows PowerShell,
with two differences:

- A line ending in `\` continues on the next line. In PowerShell, end it with
  a backtick `` ` `` instead, or type the whole command on one line.
- Setting a variable is `export NAME=value` in a Linux or macOS terminal and
  `$env:NAME = 'value'` in PowerShell. It lasts until the window is closed.

Words in capitals, such as `REPO`, `FOLDER` and `ID`, stand for your own
values. A snapshot `ID` is always the full 64-character ID; shortened IDs
such as `8ccb64f0...860a` in this guide only save space.

Every command also documents itself: run `gutch-cli --help`, or add `--help` to
any command, such as `gutch-cli installation repair --help`.

People who work on Gutch itself start with the [developer guide](../_dev_guide/README.md).
