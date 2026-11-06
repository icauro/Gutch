# Gutch CLI Usage

Canonical current-state documentation lives in [`_docs/current_state/`](_docs/current_state/README.md):

| Doc | Topic |
|---|---|
| [`README.md`](_docs/current_state/README.md) | Index, terminology, and component boundaries |
| [`01_operation_ownership.md`](_docs/current_state/01_operation_ownership.md) | Operation ownership and component boundaries |
| [`02_repository_snapshots_and_tags.md`](_docs/current_state/02_repository_snapshots_and_tags.md) | Repo metadata, free-form tags, and snapshot identity |
| [`03_snapshot_source_ingestion.md`](_docs/current_state/03_snapshot_source_ingestion.md) | Source selection and snapshot identity |
| [`04_scan_pipeline.md`](_docs/current_state/04_scan_pipeline.md) | Source inventory, chunking, reuse, and capture parallelism |
| [`05_packaging_and_bundles.md`](_docs/current_state/05_packaging_and_bundles.md) | Bundle and loose-chunk artifact contracts |
| [`06_reconstruct_pipeline.md`](_docs/current_state/06_reconstruct_pipeline.md) | Reconstruction jobs, staging, cache, and remote fetch overlap |
| [`07_verify_pipeline.md`](_docs/current_state/07_verify_pipeline.md) | Verification modes, hashing, and worker scheduling |
| [`08_installation_verification_and_repair.md`](_docs/current_state/08_installation_verification_and_repair.md) | Installation, verification, and repair |
| [`09_snapshot_runtime_metadata.md`](_docs/current_state/09_snapshot_runtime_metadata.md) | Snapshot runtime metadata |
| [`10_cli_and_app_integration.md`](_docs/current_state/10_cli_and_app_integration.md) | CLI and desktop app integration |
| [`11_path_safety_and_conflicts.md`](_docs/current_state/11_path_safety_and_conflicts.md) | Path safety and conflict resolution |
| [`12_performance_and_development.md`](_docs/current_state/12_performance_and_development.md) | Runtime planning, profiling, and benchmark interpretation |

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
`repo pull`, `repo remote list`, `repo remote unbind`, and `repo tags references`.
The generic `--scope-id` is opaque to Gutch; the provider defines its meaning.
See the [provider integration contract](_docs/current_state/10_cli_and_app_integration.md).

`repo rename` changes the local `repository_name` in `repo_metadata.json`; it
does not rename the filesystem directory or a linked remote repo.

Automation and CI: pass `--yes` to skip supported confirmations. Use
`--dry-run` for previews, including `snapshot capture --discover-from PATH`.

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
[`09_snapshot_runtime_metadata.md`](_docs/current_state/09_snapshot_runtime_metadata.md).

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
gutch-cli repo index rebuild --repo /path/to/repo
gutch-cli repo index rebuild --repo /path/to/repo --apply
```

Tag replacement removes omitted keys. Index rebuild reports by default; `--apply`
replaces the local index only after successful verification. Initialize or replace
repository constraints from JSON with `--constraints-file FILE`.

`tree compare --expected SOURCE --actual INSTALLATION` compares physical trees;
`snapshot diff` compares stored snapshot trees without reconstructing them.
