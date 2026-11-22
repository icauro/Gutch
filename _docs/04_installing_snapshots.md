# Installing snapshots

All installation commands take the repository, the snapshot ID and the
installation folder:

```sh
gutch-cli installation COMMAND --repo REPO --snapshot ID --destination FOLDER
```

## Reconstruct

```sh
gutch-cli installation reconstruct --repo REPO --snapshot ID --destination FOLDER
```

Gutch writes the complete tree into a staging folder, checks every file
against the snapshot, and then swaps it into place. If anything fails, the
staging folder is removed and the destination is left as it was. The
destination's previous content, including files the snapshot does not
contain, is replaced.

The staging folder is created next to the destination, so the two share a
disk and the final swap is quick. Use `--staging-dir FOLDER` to put it
elsewhere on the same disk. `--dry-run` reports the plan without writing.

### Reuse an existing installation

When you update from one version to another, most content is usually already
on disk. Point Gutch at the old installation and it copies matching content
from there instead of decompressing it from the repository:

```sh
gutch-cli installation reconstruct --repo REPO --snapshot NEW_ID --destination app-1.1 \
  --reuse-snapshot OLD_ID --reuse-dir app-1.0
```

Gutch checks reused content against the hashes it expects, so a damaged old
installation cannot spoil the new one.

### Download while reconstructing

If the repository does not have the snapshot yet, Gutch can download it and
install at the same time, starting to write files before every download has
finished:

```sh
gutch-cli installation reconstruct --repo REPO --snapshot ID --destination FOLDER --fetch \
  --snapshot-metadata-url URL --blueprint-url URL --artifact-url-manifest urls.json
```

Here `ID` must be the full 64-character ID, since the snapshot is not stored
yet. The URLs come from wherever the repository is published, such as
presigned object-storage links. Provide the artifact downloads with one of:

- `--artifact-url ARTIFACT_ID=URL`, repeated for each bundle or chunk;
- `--artifact-url-manifest FILE`, a JSON object mapping artifact IDs to URLs:
  `{ "da7d76...8b0a": "https://..." }`;
- `--bundle-downloads-endpoint URL` (with `--session-id`), an endpoint that
  returns download URLs for the bundles Gutch asks for.

Optional `--repo-metadata-url`, `--chunk-index-url` and
`--product-metadata-url` download those documents too. Downloads run four at a
time by default; `--transfer-concurrency N` changes that, and
`--transfer-policy adaptive` starts there and adjusts the count, up to
`--transfer-max-concurrency` (16), based on throughput, retries and how fast
the files are written.

To download a snapshot into the repository without installing it, use
`snapshot fetch` with the same options. `snapshot fetch --metadata-only`
downloads only the snapshot's metadata, blueprint and the chunk index, no
bundles. That is enough to verify an installation of the snapshot, or to
reuse its files when updating it to another snapshot; repairing it still
needs a full fetch. The library function is `fetch_snapshot_metadata`.

## Verify

```sh
gutch-cli installation verify --repo REPO --snapshot ID --destination FOLDER
```

Verify reads every file and compares its hash with the snapshot. It reports
missing and damaged entries, and exits with status 1 if it finds any. Extra
entries, which the snapshot does not contain, are listed too but do not fail
verification, because repair keeps them by default. `--strict` also checks each chunk inside every file, which
pinpoints damage further but takes longer. Verify never changes anything.

## Preview

```sh
gutch-cli installation preview --repo REPO --snapshot ID --destination FOLDER
```

Preview shows, entry by entry, what a repair would do: `keep`, `add`,
`replace`, `updateMetadata`, `remove` or `preserveExtra`, then the artifacts it would read and
the staging space it needs. It never changes the installation.

| Option | Effect |
| --- | --- |
| `--mode replace-directory` | Preview a full reconstruct instead of a repair; unchanged files still show as `keep`, though they are copied into the rebuilt folder |
| `--remove-extra-files` | Preview removing entries the snapshot does not contain |
| `--metadata-only` | Compare names, sizes and flags only; much faster, but cannot detect changed content of the same size |
| `--reuse-snapshot`, `--reuse-dir` | Account for content reused from another installation |

A preview is advice, not a reservation: the actual command inspects the
installation again before changing it.

## Repair

```sh
gutch-cli installation repair --repo REPO --snapshot ID --destination FOLDER --yes
```

Repair checks every file, then rewrites only the missing and damaged entries,
one at a time. Healthy files are not touched, which makes repair much faster
than reconstruction when little is wrong. Extra files are kept unless you add
`--remove-extra-files`. `--dry-run` shows the plan only.

Unlike reconstruction, repair replaces entries one by one: if it is
interrupted, some entries are already fixed and others are not. Run it again
to finish.

## Speed and memory

Installation commands use all CPU cores and up to a quarter of usable memory
by default. `--jobs N` limits the worker threads and `--memory-budget-mib MIB`
the working memory. Reconstruction and repair print the write speed while
they run.

## What is installed

Installations contain files, folders and symbolic links that point inside the
installation. Files keep their executable and read-only flags; folders keep
their read-only flag. Ownership, other permissions, timestamps and extended
attributes are not restored. Every installed file is a separate copy; hard
links are not recreated.
