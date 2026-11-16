# Gutch CLI Usage

The [documentation index](_docs/README.md) lists the pages that describe how
Gutch works, grouped by topic.

## Quick start

Filesystem arguments accept absolute paths or paths relative to the invocation
directory. The examples use Linux paths; Windows drive paths also work.

```sh
gutch-cli repo init --repo /home/user/repos/my-repo
gutch-cli repo rename --repo /home/user/repos/my-repo --name "Windows release"
gutch-cli snapshot capture --source /home/user/builds/1.0.0-windows-x86_64-base --repo /home/user/repos/my-repo
```

The standalone executable runs local artifact commands directly. Embedded hosts
can supply a remote command provider for `repo remote bind`, `repo remote create`, `repo push`,
`repo pull`, `repo remote list`, `repo remote unbind`, and `repo remote tag-references`.
The generic `--scope-id` is opaque to Gutch; the provider defines its meaning.
See the [provider integration contract](_docs/05_integration/01_cli_and_app_integration.md).

`repo rename` changes the local `repository_name` in `repo_metadata.json`; it
does not rename the filesystem directory or a linked remote repo.

Automation and CI: pass `--yes` to skip supported confirmations; `--json` and
`--json-lines` require it for commands that would prompt. Commands that change
state accept `--dry-run` to show their plan without applying it, including `snapshot capture --discover-from PATH`. Declined
confirmations exit with status 1 and usage errors with status 2.

## Chunker selection

Capture defaults to FastCDC. Pass `--chunker jc` for Plakar-compatible jump
chunking, or `--chunker vram256` for RAM with AVX2 acceleration and scalar fallback.
`--chunk-preset` controls the size target; for RAM it denotes the initial search
window. `17_gutch_fastcdc_jc_vram256` compares all three algorithms.
Reconstruction and repair automatically use the stored chunk references.

Run the full-flow comparison from this directory:

```sh
python gutch.py benchmark --fixture 16_gutch_fastcdc_vs_jc --product /path/to/builds
```

The product directory contains versioned build folders. The fixture compares
FastCDC at 128 KiB and JC at 256 KiB, including reconstruction and corruption repair.

## Launch options

The JSON formats and inheritance behavior are defined in
[`02_snapshot_runtime_metadata.md`](_docs/02_repository/02_snapshot_runtime_metadata.md).

## Preview installation changes

```sh
gutch-cli installation preview --repo /path/to/repo --snapshot SNAPSHOT_ID --destination /path/to/install
gutch-cli installation preview --repo /path/to/repo --snapshot SNAPSHOT_ID --destination /path/to/install --metadata-only --remove-extra-files
```

The first command hashes existing content and preserves extras in its proposed
selective repair. The second shows unverified metadata candidates and proposed
extra removal. Both leave the installation alone. Output is readable by default; add
`--json-lines` to stream events followed by a summary, or `--json` for one object.
Use `--mode replace-directory --remove-extra-files` to preview complete
replacement. Optional `--reuse-snapshot` and `--reuse-from` account for verified
reuse. Execution reinspects inputs; previews are not saved executable plans.

`snapshot measure` measures a fresh capture using the default packaging presets and reports
unique chunk bytes/count and encoded artifact bytes/count. It does not estimate
free disk space, fragmentation, or cache hit rates.

## Reconstruct and verify an installation

```sh
gutch-cli installation reconstruct --repo /path/to/repo --snapshot SNAPSHOT_ID --destination /path/to/install
gutch-cli installation verify --repo /path/to/repo --snapshot SNAPSHOT_ID --destination /path/to/install
gutch-cli installation repair --repo /path/to/repo --snapshot SNAPSHOT_ID --destination /path/to/install --yes
```

Reconstruction replaces the whole destination, including extra entries. Repair
preserves extras unless `--remove-extra-files` is supplied. Verification hashes
full files by default; `--strict` also checks each chunk hash.

Add `--fetch --snapshot-metadata-url URL --blueprint-url URL` to reconstruction
when supplying remote snapshot data. Supply artifact URLs through `--artifact-url`
or `--artifact-url-manifest`, or an authorized `--bundle-downloads-endpoint`.
Use `snapshot fetch` to populate a repository without reconstructing an installation.

## Replace tags and rebuild the index

```sh
gutch-cli snapshot tags replace --repo /path/to/repo --snapshot SNAPSHOT_ID --tag os=windows --tag arch=x86_64 --yes
gutch-cli repo index rebuild --repo /path/to/repo --dry-run
gutch-cli repo index rebuild --repo /path/to/repo
```

Tag replacement removes omitted keys. Index rebuild replaces the local index only
after successful verification; `--dry-run` reports without replacing it. Initialize or replace
repository constraints from JSON with `--constraints-file FILE`.

`debug compare-trees --expected SOURCE --actual INSTALLATION` compares physical trees;
`snapshot diff` compares stored snapshot trees without reconstructing them.
