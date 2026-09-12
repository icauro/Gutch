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
backend derives product identity from that authorized remote repo ID.
The source-folder association is also device-local, but lives in the shared
Gutch configuration rather than either repo metadata document.

`chunk_map.gtcbin` is the repo-wide chunk index. It begins with 16-byte zero-padded `GUTCH_INDEX`
and carries its binary format version.

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

`snapshot_blueprint.gtcbin` is the authoritative version-2 filesystem and
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

Repository and snapshot metadata remain at `1.0.0`, as does product runtime
metadata. Binary format versions are unchanged. During prerelease, identity
changes are applied directly without a format bump or compatibility layer.
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

Snapshot tags use `name:value` for scoped values and `name::value` for
exclusive values. Tag names use lower-snake-case (`^[a-z][a-z0-9_]*$`), and a
name can belong to only one kind. Gutch accepts explicitly supplied tags and
validates only their structural grammar. Tags are optional for local snapshots
and are completely free-form within that grammar. Gutch does not own a
repository tag schema, defaults, templates, or inference handlers.

The `version` tag has one platform-specific rule: the backend requires a
non-empty `version` tag before a snapshot can be stored remotely. Local
tagless snapshots remain valid, but they cannot be pushed or delivered until a
version tag is explicitly assigned.

## Synchronization Ownership

Creator push treats repo metadata, the chunk index, every snapshot's
metadata and blueprint, optional snapshot product metadata, and the complete
bundle-reference set as first-class artifacts. Backend repo and snapshot
records persist their BLAKE3 identities. The backend reconciles remote object
storage and removes remote bundles no longer referenced by the complete pushed
state.

Pull compares local and backend-recorded artifact hashes and applies the
selected conflict policy. Local repo linkage remains local-only and is
not part of that artifact set.

When a repo is linked, Gutch can inspect tag references supplied by its remote
provider. The read-only result is advisory; it is not compared with local tags
and never blocks capture or synchronization.

## Failure Boundaries

- JSON metadata cannot override binary blueprint filesystem facts.
- Local repo linkage cannot assert product ownership.
- Malformed tags or corrupt binary formats fail validation. Missing `version`
  is allowed locally and is rejected only by the backend remote-snapshot
  contract.
- Optional runtime metadata is synchronized independently and is removed
  remotely when the local optional file is removed and the change is pushed.
