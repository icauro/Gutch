# Getting started

This page stores two versions of a small program in a repository, installs
one, damages the installation and repairs it. It takes a few minutes.

## Get the program

Gutch is a single executable, `gutch-cli` (`gutch-cli.exe` on Windows). Either:

- **Download a release.** From the project's
  [latest release](https://github.com/icauro/Gutch/releases/latest), download
  the executable for your system, such as `gutch-cli-0.2.0-linux-x86_64` or
  `gutch-cli-0.2.0-windows-x86_64.exe` (Linux or Windows, x86-64 or ARM64;
  most PCs are x86-64), and rename it `gutch-cli` (`gutch-cli.exe` on
  Windows). `SHA256SUMS` lists each file's checksum; check yours with
  `sha256sum --ignore-missing -c SHA256SUMS`. On Linux, make it executable
  with `chmod +x gutch-cli`. The executables are unsigned, so Windows may warn
  before running one. `gutch-cli licenses` prints the licenses of everything
  built into it.
- **Build it yourself.** Install Rust 1.100 or newer, then run this from the
  project folder:

  ```sh
  cargo build --release -p gutch-cli
  ```

  The executable is written to `.builds/compiler-cache/cargo/release/`.

Put the executable in a folder on your `PATH`, the list of folders the
terminal searches for programs, or run it by its full path, such as
`C:\tools\gutch-cli.exe`, or as `./gutch-cli` from its own folder. Check
that it runs:

```sh
gutch-cli --version
```

## Store two builds

Suppose you have two builds of your program, each in its own folder:

```text
builds/
  1.0/   bin/app  readme.txt
  1.1/   bin/app  readme.txt   (readme.txt changed)
```

Create a repository, then capture each build as a snapshot. A tag such as
`version=1.0` labels the snapshot so you can tell snapshots apart later.

```sh
gutch-cli repo init --repo my-repo
gutch-cli snapshot capture --repo my-repo --source builds/1.0 --tag version=1.0 --yes
gutch-cli snapshot capture --repo my-repo --source builds/1.1 --tag version=1.1 --yes
```

Each capture prints the snapshot's ID, a 64-character name derived from the
folder's content:

```text
Captured snapshot 8ccb64f0af7148f8eb0e630bae18867f975590b61bcdf71d7b73b2d1e5bb860a.
```

`--yes` skips the confirmation question. Leave it out to see the plan and
answer `y` yourself, or use `--dry-run` to see the plan without storing
anything. The second capture stores only the content that differs from the
first; unchanged content is shared.

List what the repository holds:

```sh
gutch-cli snapshot list --repo my-repo
```

Each line shows a snapshot ID and its tags:

```text
8ccb64f0d1…860a  version=1.0
c41e09a7b2…3f1d  version=1.1
```

Commands accept the full ID or its first few characters, as long as no
other snapshot starts the same way; `8ccb` would do here.

## Install a snapshot

Reconstruct version 1.0 into a folder. Gutch builds the tree in a staging
folder next to the destination, checks every file, and only then puts it in
place, so a failure never leaves a half-written installation behind.

```sh
gutch-cli installation reconstruct --repo my-repo --snapshot 8ccb --destination installed-app
```

(Replace `8ccb` with the start of your own ID.) Reconstruction replaces the
whole destination: anything already in that folder is removed, so never
point it at a folder holding other files you want to keep.

## Check and repair it

Verify that the installation still matches the snapshot:

```sh
gutch-cli installation verify --repo my-repo --snapshot 8ccb64f0...860a --destination installed-app
```

```text
verification passed for /home/me/installed-app
```

Now damage it: delete `bin/app`, edit `readme.txt` and add a file
`extra.txt`. Verification lists each problem and exits with status 1. It
also lists the extra file, but files you add do not count as damage:

```text
1 extra entry not in the snapshot (kept):
  - extra filesystem entry: extra.txt
2 verification errors:
  - missing file: bin/app
  - size mismatch for readme.txt: expected 6, got 4
```

Ask what a repair would do, without changing anything:

```sh
gutch-cli installation preview --repo my-repo --snapshot 8ccb64f0...860a --destination installed-app
```

```text
keep             "bin"
add              "bin/app"
replace          "readme.txt"
preserveExtra    "extra.txt"
artifact         "da7d7602595b0718818d1227e58a6634742c405b7ac6144a9982caf9fa788b0a"
3 entries; 1 artifact; 300006 bytes staging
```

Then repair it:

```sh
gutch-cli installation repair --repo my-repo --snapshot 8ccb64f0...860a --destination installed-app --yes
```

Repair rewrites only the missing and damaged files and leaves healthy ones
alone. It keeps `extra.txt` because you might need it; add
`--remove-extra-files` to delete files that are not part of the snapshot.
`installation verify` now passes, and still lists the extra file.

## Next steps

- [Concepts](02_concepts.md) explains what a repository, snapshot and
  installation are.
- [Capturing snapshots](03_capturing_snapshots.md) and
  [installing snapshots](04_installing_snapshots.md) cover every option of
  the commands used above.
- [Scripting and output](06_scripting_and_output.md) shows how to use Gutch
  from scripts and CI.
