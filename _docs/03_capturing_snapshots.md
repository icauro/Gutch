# Capturing snapshots

## Capture a folder

```sh
gutch-cli snapshot capture --repo REPO --source FOLDER --tag version=1.2.0 --tag os=linux
```

Gutch shows the plan and asks for confirmation; `--yes` skips the question
and `--dry-run` shows the plan only. Capture reads every file, splits it into
chunks and stores the chunks the repository does not already have. The
snapshot appears in the repository only after everything it needs is stored,
so an interrupted capture leaves the repository as it was.

- `--source` accepts a folder or a single file and may be repeated to capture
  several snapshots in one run. `--tag` values apply to each of them.
- Archives such as `.zip` files are captured as ordinary files, not unpacked.
  Extract them first if you want their contents as the snapshot.
- If a file changes while it is read, Gutch rescans once; if it changes
  again, capture fails without storing anything. Capture from a folder that
  nothing else is writing to.
- Capturing a tree that is already stored, with the same tags, reports the
  existing snapshot and restores any of its content that had gone missing.
  With different tags it fails; change tags with `snapshot tags replace`
  instead.
- In a copy pulled with `repo pull --metadata-only`, add
  `--reuse-remote-bundles` so content stored in bundles that only the bound
  remote holds is reused instead of stored again. A later `repo push` uploads
  only the new bundles; see
  [remote synchronization](05_managing_repositories.md#remote-synchronization).

To see how much space a folder would take without a repository, run:

```sh
gutch-cli snapshot measure --source FOLDER
```

It reports the file count, the logical size, the unique chunk bytes and the
compressed size with the default settings.

## Capture every build in a folder

If one folder holds a subfolder per build, capture them all at once:

```sh
gutch-cli snapshot capture --repo REPO --discover-from builds --yes
```

The first run writes `builds/.gutch_source_config.json`:

```json
{
  "ingestion_mode": "auto",
  "auto_ingestion_rules": { "ignore_folders": [] },
  "explicit_ingestion_rules": { "snapshots": [] }
}
```

In `auto` mode every immediate subfolder becomes a snapshot. `ignore_folders`
skips subfolders whose names match a pattern; `*` and `?` are wildcards.

In `explicit` mode only listed folders are captured. They may be nested at any
depth, and each entry can carry its own tags:

```json
{
  "ingestion_mode": "explicit",
  "explicit_ingestion_rules": {
    "snapshots": [
      { "path": "windows/1.2.0", "tags": ["os=windows", "version=1.2.0"] },
      { "path": "linux/1.2.0", "tags": ["os=linux", "version=1.2.0"] }
    ]
  }
}
```

Paths are relative to the discovery folder and cannot leave it. Folder names
never become tags by themselves. Tags given with `--tag` take precedence over
tags in the file.

## Storage options

The defaults suit most software. Change them only when measurements show a
benefit.

| Option | Default | Choices |
| --- | --- | --- |
| `--chunker` | `fastcdc` | `fastcdc`; `jc` (compatible with Plakar's jump chunking); `vram256` (uses AVX2 when available) |
| `--chunk-preset` | `128KiB` | `512B` to `4MiB`; a target, not a fixed size. Smaller chunks find more shared content but cost more index space |
| `--bundle-preset` | `8MiB` | `4MiB` to `128MiB` |
| `--compression-algorithm` | `zstd` | `zstd`, `lz4` (faster, larger), `none` |
| `--compression-level` | the algorithm's default | an integer the algorithm accepts |
| `--storage-mode` | `bundled` | `bundled`, `loose-chunks` (one file per chunk) |

Installation reads the settings stored with each snapshot, so snapshots
captured with different settings install the same way.

### Bundled or loose chunks

| | Bundled (default) | Loose chunks |
| --- | --- | --- |
| Storage | Chunks packed into bundles of about 8 MiB | One file per chunk, about 128 KiB on average |
| Number of files | Few | One per chunk, so tens of thousands for a large app |
| Cost on S3 | One request per bundle | One request per chunk, and S3 charges per request |
| Download speed | High: few, large requests | Lower: the cost of each request adds up |
| Bytes from other versions | An install may fetch a whole older bundle for a few of its chunks, so it also receives content that only an older version uses | None: an install fetches exactly the chunks its snapshot uses |
| Space freed by `snapshot delete` | A bundle is removed only when no remaining snapshot uses any chunk in it | Every chunk no remaining snapshot uses is removed |
| Compression | Each bundle is compressed as a whole, which usually saves more | Each chunk is compressed on its own |
| Push, pull and remote download | Supported | Not yet: only bundles are published |

Use bundled storage for anything you ship. A bundle holds only content that
was new when it was written, so an install never receives content captured
after its snapshot, but it can receive content captured before or alongside
it. If different
users may see different snapshots of one repository and that matters to you,
keep them in separate repositories. Loose chunks suit local testing and
measurements.

## List, compare and inspect

```sh
gutch-cli snapshot list --repo REPO
gutch-cli snapshot diff --repo REPO --from OLD_ID --to NEW_ID
```

The list shows each snapshot ID with its tags. Wherever a command takes a
snapshot ID, the first few characters are enough when no other snapshot
starts the same way.

`snapshot diff` compares two stored snapshots without installing them:

```text
modified                     +3 B  "readme.txt"
1 changes; total +3 B
```

Add `--json` to `snapshot list` for each snapshot's tags, file list, size and
whether all its content is present.

## Change tags

```sh
gutch-cli snapshot tags replace --repo REPO --snapshot ID --tag version=1.2.0 --tag channel=beta
```

This replaces the whole tag set; keys you leave out are removed. To change
many snapshots at once, pass `--tags-file FILE` with a JSON object mapping
snapshot IDs to their complete new tag sets:

```json
{
  "8ccb64f0...860a": { "key_value": { "version": "1.0", "channel": "stable" } },
  "74aaf4df...0bbd": { "key_value": { "version": "1.1", "channel": "beta" } }
}
```

All replacements succeed together or none is applied.

## Delete a snapshot

```sh
gutch-cli snapshot delete --repo REPO --snapshot ID --dry-run
gutch-cli snapshot delete --repo REPO --snapshot ID --yes
```

Deleting removes the snapshot and any stored content that no other snapshot
uses. Content shared with other snapshots stays. Deletion cannot be undone.
