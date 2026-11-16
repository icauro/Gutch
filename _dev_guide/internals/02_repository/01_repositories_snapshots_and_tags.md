# Gutch Repos, Snapshots, and Tags

Status: Current

Scope: Gutch component current state

## Repo Layout and Authority

A Gutch repo uses a fixed engine-owned layout. Its principal artifacts
are:

```text
repo_metadata.json
local_repo_config.json
chunk_map.gtcbin
snapshots/<snapshot-id>/snapshot_metadata.json
snapshots/<snapshot-id>/snapshot_product_metadata.json   optional
snapshots/<snapshot-id>/snapshot_blueprint.gtcbin
bundles/<first-two-hash-characters>/<bundle-hash>
chunks/<hash-prefix>/<chunk-hash>                        loose mode only
```

`repo_metadata.json` owns repo presentation and the packaging profile.
`local_repo_config.json` is device-local and stores
only the globally unique linked `remote_repo_id`; it is never pushed. The
remote service derives product identity from that authorized remote repo ID.
The source-folder association is also device-local, but lives in the shared
Gutch configuration rather than either repo metadata document.

`chunk_map.gtcbin` is the repo-wide chunk index. It begins with 16-byte zero-padded `GUTCH_INDEX`
and carries its binary format version. The writer validates and decodes each
distinct bundle ID once, using canonical hexadecimal ordering for the artifact
table. Every chunk entry still receives size and storage-mode checks, and the
completed encoded index passes the reader's invariants before atomic publication.

Bundle files are sharded under the first two lowercase hexadecimal characters
of their 64-character content hash. The shard is derived from the bundle ID;
it is not stored separately in snapshot metadata or the chunk index.

## Snapshot Artifacts

Each snapshot represents one concrete installable variant. Local and
synchronized snapshots use the same contracts.

Snapshot artifacts are assembled in a temporary sibling directory and
promoted as one directory replacement. This prevents a failed metadata,
product-metadata, or blueprint write from exposing a mixed snapshot state.

`snapshot_metadata.json` is the lightweight JSON document for snapshot
contract version, capture metadata, and normalized tags. It does not duplicate
filesystem entries, sizes, properties, links, hashes, or chunk references.

`snapshot_blueprint.gtcbin` is the authoritative version-1 filesystem and
chunk blueprint. It begins with 16-byte zero-padded `GUTCH_BLUEPRINT` and carries its binary format version.
It records regular files, explicit directories including empty directories,
safe snapshot-internal symbolic links, executable/read-only state, content
hashes, and chunk references. It does not preserve timestamps, ownership,
ACLs, extended attributes, hidden attributes, or complete Unix modes.

`snapshot_product_metadata.json` is optional and owns runtime behavior. Its
absence means the snapshot has no product launch behavior, not that the generic
snapshot is invalid.

## Installation Identity and Format

Snapshot IDs are lowercase BLAKE3 hashes of the installed tree. Identity includes
relative paths, entry types, full-file content hashes, executable/read-only flags,
explicit directories, and symbolic-link targets. File size remains in the
blueprint for reconstruction and validation but is redundant in identity.
Chunk references, compression, bundle placement, timestamps, tags, repository
names, and absolute source locations do not contribute to the ID.

Entries are sorted lexicographically by their stored relative paths, using `/`
separators. The identity encoding starts with `gutch-installation-id-v2` and a
little-endian u64 entry count. Strings have little-endian u64 byte lengths;
entry kinds and supported attribute flags have explicit byte markers. The
scanner's existing full-file hashes are reused; identity does not reread files.
Already sorted entries avoid a second allocation and sort.

Repository, snapshot and product runtime metadata are at `document_version`
`1.0.0`, and repositories declare format 1; see
[format evolution](04_format_evolution.md) for markers and refusal rules. Binary
format versions are unchanged. During prerelease, identity changes are applied
directly without a format bump or compatibility layer.
Repositories captured with the previous identity algorithm must be recaptured
from source; blueprint identity validation still rejects mismatched IDs.

A repository stores one blueprint for each installation ID. Recapture reuses an
available representation. If artifacts are missing, capture can publish a new
complete blueprint and its artifacts while retaining descriptive metadata.
Local loads and remote imports validate blueprint entries against the ID.
Capture's availability check does not replace explicit strict artifact verification.

Fetch and pull retain a strictly verified local representation when an equivalent
remote snapshot uses a different blueprint. Imported chunk maps preserve available
local and local-only mappings, so retained snapshots remain reconstructable.
Push continues to synchronize the complete local representation, including its
blueprint, chunk map, and artifacts; snapshot ID equality alone is not a storage
representation comparison.

## Snapshot Tags

Snapshot metadata stores `"tags":{"key_value":{"version":"1.2.3.0"}}`.
The named collection remains present when empty. Compact input uses `key=value`;
only the first equals sign is a separator. Generic values preserve Unicode,
case, internal whitespace, commas and additional separators. Keys use ASCII
lower snake case; surrounding Unicode whitespace is trimmed. Empty values and
hyphens are rejected. Identical repeated assignments collapse; conflicts fail.

Repository `constraints` optionally contains `snapshot_tags.key_value` rules
and the separate `require_matching_tag_keys_across_snapshots` boolean. Each key
rule has optional `required` (false by default) and `format`. Supported formats
are `u32.u32.u32.u32` (four canonical decimal components, no leading zeroes)
and `yyyy_mm_dd` (real Gregorian dates, years 0001–9999). Unlisted keys are allowed.
Parity compares key sets within the repository; values may differ. Empty repos
have no remembered key set. Existing filesystem `targets` remain independent.

Standalone Gutch initializes without constraints. Hosts supply their own rules.
Capture, imports, tag changes and constraint updates validate before publication;
failed changes preserve the prior repository. `snapshot tags replace --tags-file PATH`
accepts a JSON object mapping snapshot IDs to complete `tags` sets and applies
all replacements atomically. `--dry-run` previews without writing. This permits
adding or removing a key across all snapshots when parity is enabled.

## Synchronization Ownership

Push treats repo metadata, the chunk index, every snapshot's
metadata and blueprint, optional snapshot product metadata, and the complete
bundle-reference set as first-class artifacts. The remote service
persists repo and snapshot records with their BLAKE3 identities and reconciles remote object
storage and removes remote bundles no longer referenced by the complete pushed
state. Push uploads only the bundles local snapshots reference and fails before
any upload when one of them is missing locally.

Pull compares local and remotely recorded artifact hashes and applies the
selected conflict policy. Every downloaded artifact must match a recorded
hash; a remote record without one is an error, not a skip. Snapshots whose
remote artifacts are not ready yet are skipped until a later pull. Local repo linkage remains local-only and is
not part of that artifact set.

When a repo is linked, Gutch can inspect tag references supplied by its remote
provider. The read-only result is advisory; it is not compared with local tags
and never blocks capture or synchronization.

## Failure Boundaries

- JSON metadata cannot override binary blueprint filesystem facts.
- Local repo linkage cannot assert product ownership.
- Malformed tags, corrupt binary formats and configured constraint violations
  fail validation. Missing `version` is allowed only when no rule requires it.
- Optional runtime metadata is synchronized independently and is removed
  remotely when the local optional file is removed and the change is pushed.
