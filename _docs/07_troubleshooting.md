# Troubleshooting

Add `--error-output report.json` to a failing command to get the full details
in a file; see [error reports](06_scripting_and_output.md#error-reports).

## Getting started

**`gutch-cli: command not found`**, **`'gutch-cli' is not recognized`** — the
terminal cannot find the program. Run it by its full path, or put its folder
on your `PATH`; see [get the program](01_getting_started.md#get-the-program).

**`no snapshot … has an ID starting with …`**, **`… is not a snapshot ID`** —
the ID is mistyped or belongs to another repository. Copy it, or its first
few characters, from `snapshot list`. **`2 snapshots have an ID starting
with …`** — give more characters.

## Capture

**`capture rejected: N filename compatibility violation(s)`** — the folder
contains names that a target filesystem of this repository cannot store. The
message lists each path and the reason. Rename those files, or change the
targets with `repo constraints set`.

**`field 'snapshot_tags': existing '...', requested '...'`** — this exact
content is already stored with other tags. Capture never changes a stored
snapshot. To change its tags, run `snapshot tags replace` with the snapshot ID
from the message.

**`key 'version' must match u32.u32.u32.u32`** and similar tag errors — the
repository's [tag rules](05_managing_repositories.md#tag-rules) reject the
tags. Check them with `repo constraints show`.

**`tag values cannot contain hyphens`**, **`tag key ... must use lower snake case`**
— see the tag format in [concepts](02_concepts.md#tags).

**The source changed during capture** — another program was writing to the
folder. Wait until it finishes, or capture from a copy.

## Confirmation

**`confirmation requires input`** — the command wanted to ask a question, but
nothing could answer, as in a script or CI job. Add `--yes`, after checking the
plan with `--dry-run`.

**`confirmation prompts are unavailable with --json or --json-lines`** — the
same, for structured output.

## Installation

**`verification errors`** — `installation verify` found missing or damaged
entries. Run `installation preview` to see the fix, then
`installation repair`. Extra entries are listed separately and do not fail
verification; remove them with `installation repair --remove-extra-files`.

**Reconstruction fails while swapping the folder into place** — usually
another program holds a file in the destination open. Close programs using
the installation and run the command again; the error report's `directory_promotion_failed` details say which step
failed and whether the old installation was restored.

**Repair was interrupted** — run it again. Entries already fixed are kept;
the rest are fixed on the next run.

## Repository

**`repo verify` reports missing or corrupt artifacts** — stored content was
lost or damaged. Capture the original folder again to restore it, or pull it
from a remote copy. Snapshots that use only healthy artifacts still install.

**The chunk index is missing or damaged** — run `repo index rebuild`.

**`repository at … is encrypted; unlock it with a password or key file`** —
set `GUTCH_REPO_PASSWORD` or `GUTCH_REPO_KEY_FILE` in this terminal window
first; see [encryption](05_managing_repositories.md#encryption).

**`wrong password for the encrypted repository`**, **`wrong key file …`** —
the credential does not open this repository. Check for typing mistakes and
extra spaces. A forgotten password cannot be recovered; use another
credential added with `repo credential add`.

**`encrypted repositories chunk with FastCDC`** — encrypted repositories
refuse `--chunker jc` and `vram256`. Leave `--chunker` out.

**`needs_newer_gutch` / `unsupported_format`** — the repository was written by
a newer version of Gutch. Upgrade `gutch-cli`.

**`remote commands are unavailable`** — remote commands need a server
connection that the standalone `gutch-cli` does not have; see
[remote synchronization](05_managing_repositories.md#remote-synchronization).
