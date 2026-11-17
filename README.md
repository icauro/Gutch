# Gutch

Gutch is a fast deduplicating store and installer for folder trees, written
in Rust as a library and a command-line tool. It installs 2–4× faster and
repairs 2–8× faster than Restic, Kopia, Borg, Rustic and Plakar, and stores
the same amount of data or less
([benchmarks](https://github.com/icauro/Gutch-Benchmarks)). When you capture
a folder, Gutch cuts each file into chunks at points chosen by the content
itself, names every chunk by its BLAKE3 hash, compresses it and packs it into
larger bundle files. A chunk that is already in the store is not stored
again, so a new version costs only the bytes that actually changed. When you
install, Gutch rebuilds the tree from those chunks in a
staging folder, checks every file against its hash, and only then moves it
into place.

In everyday terms, it keeps every version of a program, game, dataset or
other set of files in one place, stores each byte only once, and can put any
version on a disk quickly and correctly. You capture a folder into a
*repository*, which is the store. Each capture becomes a *snapshot* with a
long ID and labels such as `version=1.0.0`. You can then install a snapshot
anywhere, check that an installation is still intact, and repair it:

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
back any old build exactly.

It also works as a general deduplication tool for archiving many versions of
any folder, such as builds, datasets or asset libraries, when you only need
the file contents back. Gutch keeps file contents, symbolic links and the
executable and read-only flags. It does not keep owners, other permissions,
timestamps, extended attributes or hard links, and it has no retention
schedules or cloud storage backends, so it is not a full system backup tool.

## Performance

Gutch installs a snapshot 2.2–4.6× faster and repairs a damaged installation
2.4–29× faster than the deduplication engines in Restic, Kopia, Borg, Rustic
and Plakar, while storing the same amount of data or less.

[![Gutch benchmark comparisons](https://raw.githubusercontent.com/icauro/Gutch-Benchmarks/main/_docs/reports/comparison.svg)](https://github.com/icauro/Gutch-Benchmarks)

Each pair runs on one machine with settings matched as closely as each tool
allows (chunk sizes, compression, encryption and worker count), and every
restored file is checked with SHA-256. Gutch puts installing ahead of
capturing, and Kopia still captures about 1.2× faster. The
[Gutch Benchmarks](https://github.com/icauro/Gutch-Benchmarks) repository
has the full method, raw results and the remaining differences between
tools, and anyone can run it again.

### Why it is faster

- **Each bundle is read once.** Gutch plans an install around the bundles it
  needs rather than file by file. Each bundle is read and unpacked once, and
  every chunk in it is written directly to each file and position that uses
  it.
- **Nothing waits in line.** Bundles are unpacked on all cores at the same
  time, and each file is checked as soon as its own chunks are written,
  while other files are still being built. Downloads and unpacking overlap.
- **Repair touches only what is broken.** Gutch first finds the missing or
  damaged files, then reads only the bundles those files need and replaces
  only those files. Healthy files are not read a second time or rewritten.
- **Fast hashing.** Every chunk and file is checked with BLAKE3, which is
  several times faster than the SHA-256 that most backup tools use.
- **Less to restore.** Gutch does not keep owners, timestamps or extended
  attributes, so there is no metadata to apply file by file.
- **Built for installing.** A snapshot is captured once and installed many
  times, so whenever capture speed and install speed conflict, Gutch
  chooses install speed.

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

Download `gutch-cli` for Linux or Windows on x86-64 or ARM64 from the
[latest release](https://github.com/icauro/Gutch/releases/latest). Each
release includes `SHA256SUMS` so you can check the download. To build from
source with Rust 1.100 or newer:

```sh
cargo build --release -p gutch-cli
```

## License

Original project code is [MIT licensed](LICENSE). Included third-party source
retains its own terms; see [third-party notices](THIRD_PARTY_NOTICES.md).
