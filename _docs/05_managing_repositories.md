# Managing repositories

## Create and inspect

```sh
gutch-cli repo init --repo REPO
gutch-cli repo status --repo REPO
gutch-cli repo rename --repo REPO --name "Windows release"
```

`repo init` asks before using a folder that already has content, and removes
any Gutch repository files already in it; `--force` skips the question.
`repo status` shows the name, the bound remote repository, and
the number and size of stored snapshots and artifacts. `repo rename` changes
only the display name stored in the repository, not the folder name or any
remote copy.

## Encryption

Repositories are not encrypted unless asked, and a plain repository does no
extra work. An encrypted one stores file content, file names, tags and the
chunk index sealed with AES-256-GCM, and names its files so that they do not
reveal which content is present:

```sh
export GUTCH_REPO_PASSWORD='a long passphrase'   # PowerShell: $env:GUTCH_REPO_PASSWORD = 'a long passphrase'
gutch-cli repo init --repo REPO --encrypt
gutch-cli snapshot capture --repo REPO --source build --tag version=1.0
```

Later commands in the same window use the password without asking. In a new
window, set it again first.

Credentials come from the environment, never from the command line:
`GUTCH_REPO_PASSWORD` holds a password, or `GUTCH_REPO_KEY_FILE` names a key
file. Every command that reads or writes an encrypted repository needs one;
a plain repository ignores them. `repo init --new-key-file FILE` encrypts with
a new random key file instead of a password. Passwords are stretched with
Argon2id, so a key file unlocks faster.

More credentials can open the same repository. Unlock it with an existing
one, then add a password from `GUTCH_REPO_NEW_PASSWORD` or a new key file:

```sh
GUTCH_REPO_NEW_PASSWORD='second passphrase' gutch-cli repo credential add --repo REPO --new-password
gutch-cli repo credential add --repo REPO --new-key-file team.key
```

A lost credential cannot be recovered, and nobody can open the repository
without one: keep a key file or a written-down password somewhere safe, apart
from the repository. Copies
made by push, pull and fetch share the repository's key, so the same
credentials open them, and a remote stores only sealed files and snapshot IDs,
no tags.

Encrypted repositories chunk with FastCDC, using cut points derived from the
key, so chunk sizes do not reveal content either. Encryption hides content,
names and tags, but not the repository's display name, the number and sizes
of its files, or when they were written. `bundle inspect` reads only bundles
from plain repositories.

## Check stored content

```sh
gutch-cli repo verify --repo REPO
gutch-cli repo verify --repo REPO --strict
```

The default check confirms that every snapshot's metadata is consistent and
that every artifact it needs is present and well-formed. `--strict` also
decompresses every artifact and checks each chunk's hash, which reads the
whole repository. Add `--snapshot ID` to check one snapshot's content (the
metadata of all snapshots is still checked).

A damaged artifact can be restored by capturing the original folder again,
which re-stores missing or damaged content, or by pulling it from a remote
copy. Gutch keeps no spare copies of its own.

## Filename rules

A repository can promise that every snapshot installs cleanly on chosen
filesystems. Gutch then rejects a capture that contains, for example, a file
named `CON.txt` when Windows is a target:

```text
error: capture rejected: 1 filename compatibility violation(s).
"x/CON.txt": windows-ntfs: reserved Windows device name
```

Choose targets when creating the repository, or change them later:

```sh
gutch-cli repo init --repo REPO --filesystem windows-ntfs --filesystem linux-ext4
gutch-cli repo constraints set --repo REPO --filesystem macos-apfs --filesystem ios-apfs
gutch-cli repo constraints show --repo REPO
gutch-cli repo constraints clear --repo REPO
```

Targets: `windows-ntfs`, `windows-exfat`, `linux-ext4`, `linux-ext4-casefold`,
`linux-btrfs`, `macos-apfs`, `macos-apfs-case-sensitive`, `ios-apfs`,
`android-ext4`, `android-f2fs`. The checks cover reserved names, forbidden
characters, name and path length limits, and names that differ only by
letter case on case-insensitive filesystems. No targets are set by default.
`constraints set` replaces the whole set, and fails if a stored snapshot
breaks the new rules.

## Tag rules

The same constraints can require tags. `repo constraints show` prints the
complete rules object; edit it and pass it back with `--constraints-file`:

```json
{
  "targets": ["windows-ntfs"],
  "snapshot_tags": {
    "key_value": {
      "version": { "required": true, "format": "u32.u32.u32.u32" },
      "release_date": { "format": "yyyy_mm_dd" }
    }
  },
  "require_matching_tag_keys_across_snapshots": true
}
```

```sh
gutch-cli repo constraints set --repo REPO --constraints-file rules.json
```

- `required` makes a key mandatory on every snapshot.
- `format` restricts its value: `u32.u32.u32.u32` is a four-part version such
  as `1.2.0.0` (no leading zeros), and `yyyy_mm_dd` is a real date such as
  `2026_11_16`.
- `require_matching_tag_keys_across_snapshots` makes every snapshot use the
  same set of keys; values may differ. To add or remove a key across all
  snapshots, use `snapshot tags replace --tags-file`, which changes them all
  at once.

Keys without a rule are allowed. Captures, tag changes and rule changes that
break the rules fail and leave the repository unchanged.

## Rebuild the index

The chunk index (`chunk_map.gtcbin`) records where each chunk is stored. If it
is lost or damaged, rebuild it from the stored artifacts:

```sh
gutch-cli repo index rebuild --repo REPO --dry-run
gutch-cli repo index rebuild --repo REPO
```

The rebuild reads and checks every artifact and replaces the index only if it
completes successfully. The report lists damaged artifacts, chunks that could
not be found and the snapshots they affect.

## Remote synchronization

A repository can be bound to a remote copy on a server:

| Command | Effect |
| --- | --- |
| `repo remote list` | List the remote repositories you can access |
| `repo remote create --repo REPO --scope-id ID` | Create a remote repository and bind to it |
| `repo remote bind --repo REPO --remote-repo-id ID` | Bind to an existing remote repository without transferring anything |
| `repo remote unbind --repo REPO` | Remove the binding; add `--delete-remote` to delete the remote copy too |
| `repo push --repo REPO` | Upload local changes to the bound remote |
| `repo pull --repo REPO` | Download remote changes; installations are not updated |
| `repo remote tag-references --repo REPO` | Show tag references the server reports |

`push` and `pull` show their plan and ask for confirmation; `--dry-run` and
`--yes` work as elsewhere. When a snapshot changed on both sides, `pull`
follows `--on-conflict`: `keep-newer` (default), `keep-local` or
`keep-remote`. `--prune` deletes local snapshots that the remote no longer
has. `--remote-repo-id` pulls from a remote other than the bound one.
`--snapshot ID`, repeatable, downloads only those snapshots and the bundles
they need. A copy pulled that way lacks the other snapshots, and a push from it
would delete them from the remote, so pull everything before you push.
`--metadata-only` downloads snapshot metadata but no bundles, enough to list
and compare files; a later pull without it downloads the bundles. Such a copy
can also publish new versions without downloading the old ones: capture with
`snapshot capture --reuse-remote-bundles`, then push. Push checks that the
remote holds each bundle missing here and uploads only the new ones.

These commands need a connection to a server, which the application that
embeds Gutch provides; its documentation says how to sign in and what
`--scope-id` means. The standalone `gutch-cli` has no server connection of its
own and reports `remote commands are unavailable` for them. See
[embedding Gutch](08_embedding_gutch.md#remote-providers).
