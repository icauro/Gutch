# Concepts

## Repository

A repository is a folder that stores snapshots. It contains:

| Path | Holds |
| --- | --- |
| `repo_metadata.json` | The repository's name, format version, packaging settings and rules |
| `snapshots/<id>/` | One folder per snapshot: its metadata and its blueprint (the list of files) |
| `bundles/` or `chunks/` | The stored file content |
| `chunk_map.gtcbin` | An index of where each piece of content is stored |
| `.gutch-lock`, `.gutch-transactions/` | Locking and crash recovery; leave them alone |

Only change a repository through `gutch-cli` or the library. Several Gutch
processes may use the same repository at once: readers share it, and a
process that changes it waits for exclusive access.

## Snapshot

A snapshot is one complete folder tree as it was captured: its files, folders,
safe symbolic links, and each file's content, executable and read-only flags.

A snapshot's **ID** is computed from that content. The same tree always gets
the same ID, no matter when or from where it was captured; capturing it again
changes nothing. Any change to content or to a stored property gives a new ID.

Snapshots do not record owners, permissions beyond the executable and
read-only flags, timestamps, extended attributes or hard links. Gutch delivers
software; it is not a full backup tool.

## Tags

Tags are `key=value` labels on a snapshot, such as `version=1.2.0` or
`os=windows`. They describe a snapshot but are not part of its ID, so you can
change them later without capturing again. A key starts with a letter and
uses ASCII letters, digits and underscores; uppercase letters are lowered.
Each key has one value. Values may contain spaces, Unicode and further `=`
signs, but must not be empty or contain hyphens. A repository can require
certain tags or value formats (see [managing repositories](05_managing_repositories.md#tag-rules)).

`version` is shown as the snapshot's version label in summaries.

## Chunks and bundles

Gutch splits file content into **chunks**, pieces averaging about 128 KiB that
are cut where the content itself suggests, so an insertion near the start of a
file does not shift every later chunk. Each chunk is named by its BLAKE3 hash
and stored once per repository, no matter how many files or snapshots use it.

Chunks are compressed and packed into **bundles** of a few megabytes, which
keeps the number of files small and downloads efficient. A repository can
store chunks as loose files instead. Bundles and loose chunks together are
called **artifacts**.

## Installation

An installation is a folder reconstructed from a snapshot. Gutch can:

| Operation | What it does |
| --- | --- |
| **Preview** | Lists what reconstruct or repair would change; changes nothing |
| **Reconstruct** | Builds the whole tree in a staging folder, verifies it, then replaces the destination with it |
| **Verify** | Compares the installation with the snapshot and reports missing, damaged and extra entries |
| **Repair** | Rewrites only missing and damaged entries; keeps extra files unless asked to remove them |

**Extra files** are entries in the installation that the snapshot does not
contain, such as user settings or logs. Reconstruction removes them; repair
keeps them by default.

## Remote repositories

A local repository can be bound to a remote copy and synchronized with
`repo push` and `repo pull`. Gutch handles comparison, transfer and local
changes; the application that embeds Gutch supplies the connection to its
server and decides who may access what. The standalone `gutch-cli` has no
built-in server connection; see [remote synchronization](05_managing_repositories.md#remote-synchronization).
