# Getting started

This page stores two versions of a small program in a repository, installs
one, damages the installation and repairs it. It takes a few minutes.

## Get the program

Gutch is a single executable, `gutch-cli` (`gutch-cli.exe` on Windows). Either:

- **Download a build.** In the project's GitHub page, open **Actions → Build
  Gutch CLI**, pick a successful run and download the package for your system
  (Linux or Windows, x86-64 or ARM64). Each package contains the executable,
  a `SHA256SUMS` file and license notices. These builds are unsigned and kept
  for 30 days.
- **Build it yourself.** Install Rust 1.100 or newer, then run this from the
  project folder:

  ```sh
  cargo build --release -p gutch-cli
  ```

  The executable is written to `.builds/compiler-cache/cargo/release/`.

Put the executable somewhere on your `PATH`, or call it by its full path.
Check that it runs:

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

Commands need the full snapshot ID; copy it from the capture output or the
list.

## Install a snapshot

Reconstruct version 1.0 into a folder. Gutch builds the tree in a staging
folder next to the destination, checks every file, and only then puts it in
place, so a failure never leaves a half-written installation behind.

```sh
gutch-cli installation reconstruct --repo my-repo --snapshot 8ccb64f0...860a --destination installed-app
```

(Replace `8ccb64f0...860a` with the full ID.) Reconstruction replaces the
whole destination: anything already in that folder is removed.

## Check and repair it

Verify that the installation still matches the snapshot:

```sh
gutch-cli installation verify --repo my-repo --snapshot 8ccb64f0...860a --destination installed-app
```

```text
verification passed for /home/me/installed-app
```

Now damage it: delete `bin/app`, edit `readme.txt` and add a file
`extra.txt`. Verification lists each problem and exits with status 1:

```text
3 verification errors:
  - missing file: bin/app
  - size mismatch for readme.txt: expected 6, got 4
  - extra filesystem entry: extra.txt
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
3 entries; 1 artifacts; 300006 bytes staging
```

Then repair it:

```sh
gutch-cli installation repair --repo my-repo --snapshot 8ccb64f0...860a --destination installed-app --yes
```

Repair rewrites only the missing and damaged files and leaves healthy ones
alone. It keeps `extra.txt` because you might need it; add
`--remove-extra-files` to delete files that are not part of the snapshot.
Until you do, `installation verify` keeps reporting the extra file.

## Next steps

- [Concepts](02_concepts.md) explains what a repository, snapshot and
  installation are.
- [Capturing snapshots](03_capturing_snapshots.md) and
  [installing snapshots](04_installing_snapshots.md) cover every option of
  the commands used above.
- [Scripting and output](06_scripting_and_output.md) shows how to use Gutch
  from scripts and CI.
