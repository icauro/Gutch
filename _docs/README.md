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

Every command also documents itself: run `gutch-cli --help`, or add `--help` to
any command, such as `gutch-cli installation repair --help`.

People who work on Gutch itself start with the [developer guide](../_dev_guide/README.md).
