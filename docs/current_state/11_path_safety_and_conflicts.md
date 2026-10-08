# Gutch Path Safety and Pull Conflicts

Status: Current

Scope: Gutch component current state

## Boundary

Gutch validates every external path before repo or install mutation,
limits operations to fixed layouts, stages replacements before promotion, and
requires explicit policy where synchronization may discard a differing local
artifact.

The engine and CLI currently support repo pull conflict policies. The
app exposes pull preview and native pull commands but does not currently
implement the former blueprint's periodic conflict check, summary modal,
grouped per-item review, or automatic resurfacing cadence.

## Path Ownership

Repo operations derive paths from the fixed repo layout instead of
accepting arbitrary artifact destinations. Metadata and runtime paths are
normalized relative paths. Absolute paths, parent traversal, backslashes,
drive prefixes, null bytes, empty components, and unexpected `.` components
are rejected.

Before touching a path beneath an owned root, Gutch canonicalizes existing
ancestors, verifies containment, and checks the expected entry kind. CLI
filesystem roots must be absolute.

Full reconstructed verification uses a rooted, no-follow filesystem inventory:
directories are containment-checked as they are discovered, expected metadata
and extra entries are collected in the same traversal, and symbolic-link
leaves are inspected without following them. Targeted post-repair verification
keeps the shared ancestor resolver for only the affected entries.

Repo IDs, snapshot IDs, bundle IDs, and chunk hashes used to derive
repo paths must be one safe path component. Gutch rejects empty values,
`.`/`..`, separators, and multi-component values before joining them to an
owned root.

## Write, Replace, and Delete Behavior

Downloads and derived files use same-directory temporary files and are
atomically replaced only after validation. A complete snapshot artifact set is
assembled in a temporary snapshot directory before it replaces the prior
snapshot directory. Directory reconstruction uses staging and verification
before promotion. A failed replacement leaves the prior committed content
authoritative or restores it from the operation's backup path.

Repository pull checks remote metadata and blueprints against provider-recorded
BLAKE3 hashes. Direct snapshot fetch validates decoded metadata and blueprints;
its URL request does not supply expected metadata hashes. Both paths check
downloaded bundles against their content-addressed IDs.

Direct fetch retains the previous snapshot while downloading and validating its
replacement. It publishes the complete snapshot directory after successful
transfer or reconstruction. Repository metadata, the chunk index, and snapshots are protected by a local
reader/writer lock and recoverable metadata-generation publication. Pending
publication fails reads explicitly and is restored by the next mutation.
Installation promotion uses a destination lock and an owned, deterministic
backup that a subsequent reconstruction or repair can restore.

Once directory promotion or repository metadata publication commits, failure to
remove obsolete content does not change the successful result.

A directory promotion cleanup failure emits a warning with structured diagnostics
on stderr, including `committed: true`. It does not change the exit status or
the selected success result. Normal successful promotion emits no diagnostic
warning. Failure diagnostics and JSON export are documented in
[CLI integration](10_cli_and_app_integration.md).

A retained owned directory backup is excluded from snapshot listings and its removal is retried
before the next promotion. Old metadata generations are retried on the next
publication; undeleted orphan artifacts remain eligible for repository cleanup.
These leftovers can temporarily consume disk space. Ownership checks and
recovery before a new mutation still fail if they cannot complete safely.

Reconstruction and repair treat `--staging-dir` as a staging parent. They create
a unique operation-owned child, preserve preexisting contents, and remove that
child on completion or failure. Staging must not overlap the destination or a
reused source, including through links or parent components. Neither staging nor
the destination may contain the repository. A newly created staging parent is
removed when empty; preexisting parents remain.

Binary metadata decoders check declared lengths and counts against remaining
input before allocating arrays or strings.

Repo initialization assesses the destination first. A non-directory is
rejected, while a nonempty directory or existing repo requires explicit
confirmation. Forced initialization removes only recognized Gutch-owned
artifacts and preserves unrelated files.

Snapshot deletion is scoped to the selected snapshot directory. Before removing
it, cleanup resolves all surviving snapshot references through the chunk index
and checks artifact availability. An incomplete index or missing required
artifact stops deletion. Cleanup removes only layout-owned artifacts that no
remaining snapshot references.

## Pull Conflict Policies

Repo pull compares local BLAKE3 identities with backend-recorded hashes.
The supported policies are:

- `keep-local` — retain a differing local artifact; missing local artifacts are
  still fetched;
- `keep-remote` — fetch a differing remote artifact;
- `keep-newer` — compare the backend artifact timestamp with local filesystem
  modification time and keep the newer side.

The CLI exposes these through `--on-conflict`. App native requests default to
`keep-newer` when no policy is supplied. Pull preview reports whether repo
metadata and the chunk map are needed, snapshots to download or skip,
local-only snapshots, local artifacts retained as newer, and bundle count.

Pruning local-only snapshots and unreferenced bundles is a separate explicit
choice. A default pull does not treat local-only content as permission to
delete it.

## Interaction Flow

1. The caller requests a pull preview with a conflict policy and prune choice.
2. The provider resolves authorized remote records and artifact sources; Gutch
   compares the stored hashes with local artifacts.
3. The preview identifies fetch, skip, retained-local, and local-only outcomes.
4. After caller confirmation, Gutch asks the provider for required sources,
   downloads artifacts to temporary paths, validates them, and promotes them.
5. Explicit pruning removes only layout-owned artifacts selected by the plan.
6. The final result reports applied, skipped, retained, and pruned counts.

## Failure Boundaries

- A malformed remote identifier or unauthorized backend response fails before
  local mutation.
- Missing or invalid remote timestamps do not authorize arbitrary deletion;
  the current newer comparison treats the remote side as current for fetch
  selection.
- The current engine plan is artifact/snapshot oriented, not the rich per-file
  conflict-card model described by the retired blueprint.
- Closing an app preview or declining CLI confirmation performs no mutation.


## Path limits and exact names

Relative manifest paths and link targets are limited to 131,072 UTF-8 bytes and
256 components. The binary string reader checks the byte limit before allocating;
the writer and capture enforce the same bound. The wire length remains `u32`.
Native path components must convert to Unicode without replacement. Capture joins
components using `/`; a literal backslash in a Unix filename is rejected rather
than reinterpreted as a directory separator. Unicode normalization is not applied
to stored names.

Before reconstruction creates staging, it checks the destination and staging
paths, including the operation directory. Windows checks 255 UTF-16 units per
component, 32,766 for the full path, reserved device names, trailing dots/spaces,
and prohibited characters. Unix checks 255 component bytes and a full-path budget
of 4,095 bytes (1,023 on macOS). These are supported platform budgets; unusual
filesystems may impose smaller limits and can still return filesystem errors.

A disposable probe on the target volume detects case and normalization behavior.
Potential aliasing paths are confirmed on that filesystem before rejection.
Names are never truncated, renamed or normalized to make an installation fit.
Read-only directories or changes to mount/case settings can still prevent the
probe or subsequent writes; errors leave installed content unpromoted.

## Optional repository filename constraints

`repo_metadata.json` may contain `constraints: {"targets": [...]}`. Missing,
null, or empty constraints impose no portability policy. UTF-8 representation,
relative-path safety and engine format limits remain mandatory. No operation
renames, normalizes or silently skips incompatible names.

Targets are OS/filesystem naming profiles:

| Target | Profile assumptions |
|---|---|
| `windows-ntfs`, `windows-exfat` | Windows reserved characters/device names, no trailing dots/spaces, 255 UTF-16 units per component, case-insensitive comparison |
| `linux-ext4`, `linux-btrfs` | Case-sensitive names, 255 UTF-8 bytes per component |
| `linux-ext4-casefold` | Linux byte budgets with canonical-normalization and conservative case collision detection |
| `macos-apfs` | Canonical-normalization and conservative case collision detection |
| `macos-apfs-case-sensitive`, `ios-apfs` | Case-sensitive names with canonical-normalization collision detection |
| `android-ext4`, `android-f2fs` | Case-sensitive app-private storage, Linux byte budgets; shared/document-provider storage is outside these profiles |

Apple profiles conservatively budget 255 UTF-8 bytes per component. Relative
path budgets are 32766 UTF-16 units on Windows, 1023 bytes for Apple profiles,
and 4095 bytes for Linux/Android profiles. These are supported Gutch profile
budgets, not a complete description of every filesystem's theoretical limits.
The final destination prefix and staging location are checked at installation.

Collision checks include ancestor directory spellings, not only complete file
paths. Profiles use Gutch's Unicode normalization/case rules; they do not emulate
every volume's versioned Unicode tables. Actual destination validation is still
required. Profiles describe filenames, not portable permissions, executable bits,
symlink support, mobile storage access or application launch support.

Capture validates the source inventory before encoding artifacts and again on a
source-change retry. Setting constraints checks existing snapshots before saving.
Snapshot publication, incoming fetch, repository verification, reconstruction and
repair enforce the repository policy. Pull transactions roll back rejected metadata;
remote constraints may not silently replace different existing local constraints.
Explicitly update local constraints before synchronizing a different remote policy.
New caches adopt validated remote constraints.

Errors carry operation, optional snapshot ID, offending path, optional conflicting
path, target, stable reason code and explanation. `actual-destination` identifies
an installation filesystem failure rather than a repository target violation.
Terminal output escapes names and shows at most ten violations; error JSON retains
all violations. Existing destinations remain unchanged when preflight rejects a path.
