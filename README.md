# Gutch

Gutch is a Rust library and command-line tool that stores folder trees and
installs them again as exact copies. When you capture a folder, Gutch cuts
each file into chunks at points chosen by the content itself, names every
chunk by its BLAKE3 hash, compresses it and packs it into larger bundle
files. A chunk that is already in the store is not stored again, so a new
version of a program costs only the bytes that actually changed. When you
install, Gutch rebuilds the tree from those chunks in a staging folder,
checks every file against its hash, and only then moves it into place.

In everyday terms, it keeps every version of a program, game or other set of
files in one place, and can put any of them on a disk quickly and correctly.
You capture a folder into a *repository*, which is the store. Each capture
becomes a *snapshot* with a long ID and labels such as `version=1.0.0`. You
can then install a snapshot anywhere, check that an installation is still
intact, and repair it:

```sh
gutch-cli repo init --repo store
gutch-cli snapshot capture --repo store --source build-1.0 --tag version=1.0.0
gutch-cli installation reconstruct --repo store --snapshot ID --destination app
gutch-cli installation verify --repo store --snapshot ID --destination app
```

Repair rewrites only the files that are missing or damaged. Gutch can also
download a snapshot and install it at the same time, encrypt a repository
with a password or key file, refuse file names that would not work on the
systems you target, and sync repositories with a server that your own
application provides.

## What it is for

Gutch is built for programs that install the same large folders over and
over: app stores and game launchers, package managers that keep many
versions side by side, asset servers that hand out big sets of files that
change a little at a time, and build archives or test labs that need to bring
back any old build exactly. It is not a backup tool. It does not keep file
owners, timestamps or extended attributes, because an installed copy does not
need them.

## How it is designed

A snapshot is usually captured once and installed many times, so installing
gets the most attention. Capture is fast and runs in parallel too, but when
the two pull in different directions, installing wins.

Gutch tries never to move or write the same bytes twice. Versions share
chunks in the store, and an update can reuse the files already on disk, so
only the differences are downloaded and written.

A snapshot's ID comes from its content. Capturing the same tree again changes
nothing, and every installation can be checked against that ID.

An installation is never left half finished. The new copy is built and
checked next to the old one, and the old one stays in place until the swap.

Features you do not use cost nothing. Encryption, storing chunks as loose
files, and profiling are all off by default and add no work when they are
off.

Gutch is a library first. `gutch-cli` is a thin layer on top of it, and an
application that embeds Gutch keeps control of its own server, accounts and
access rules.

## Documentation

The [user guide](_docs/README.md) starts with
[getting started](_docs/01_getting_started.md) and covers every command, plus
[embedding Gutch](_docs/08_embedding_gutch.md) in your own Rust application.

To work on Gutch itself, see the [developer guide](_dev_guide/README.md).

## Getting Gutch

Prebuilt `gutch-cli` binaries for Linux and Windows on x86-64 and ARM64 are
attached to successful runs of **Actions > Build Gutch CLI**. To build from
source with Rust 1.100 or newer:

```sh
cargo build --release -p gutch-cli
```

## License

Original project code is [MIT licensed](LICENSE). Included third-party source
retains its own terms; see [third-party notices](THIRD_PARTY_NOTICES.md).
