# Gutch Binary File Layouts

Status: Current

Scope: Artifact, chunk-map, and snapshot-blueprint bytes

Verified: 2026-09-12

Integers are unsigned little-endian without Rust padding. `Hash32` is a binary
BLAKE3 digest. All binary formats use version **1**. This prerelease layout is a clean replacement;
no compatibility reader exists for superseded layouts.
JSON document versions remain `1.0.0`. Snapshot IDs describe installation content
and properties; their canonical identity algorithm is independent of these bytes.

## Files and roles

| Repository path | Role |
|---|---|
| `snapshots/<id>/snapshot_blueprint.gtcbin` | Canonical tree entries and chunk sequences |
| `chunk_map.gtcbin` | Sorted chunk identities mapped to artifact locations |
| `bundles/<prefix>/<id>` | One encoded stream and its physical chunk table |
| `chunks/<prefix>/<hash>` | One self-describing loose chunk |
| `repo_metadata.json` | Capture defaults; existing artifacts carry their codec |

A blueprint describes logical files; the map selects their storage locations.
Bundles retain full physical chunk hashes and sizes so map regeneration does not
require an existing map. Metadata does not share pages across snapshots.

## Metadata envelope

Blueprints and maps use this envelope; bundles and loose chunks use fixed headers.

```text
magic[16]
version:u16 = 1; reserved:u16 = 0; group_count:u32 = G
for each group, in ascending ID order:
  id:u16; flags:u8 = 1; codec:u8     # required; raw=0, Zstd=1
  encoded_length:u32; decoded_length:u32
  encoded_digest:Hash32
header_and_directory_digest:Hash32
encoded_group_bytes[]              # consecutive, no gaps or offsets
```

Framing costs `56 + 44G` bytes. Writers use Zstd level 3 only when it shrinks a
group; otherwise they store raw bytes. Readers verify the header/directory digest
before interpreting descriptors and verify each encoded group before decoding.
Decoded lengths must match exactly. Both complete encoded metadata and the sum of
decoded group lengths are bounded to 256 MiB; the Zstd window is bounded as well.
Readers reject unsupported versions, reserved bits, codecs, group counts, IDs,
flags, truncation and trailing data.

All current groups are required. These materializing APIs also refuse unknown
optional groups: discarding an extension and later rewriting the object would
lose data. Self-contained hashes detect corruption; they do not authenticate an
attacker-supplied object. Remote callers supply the trusted whole-object digest.
Current operations decode and validate all groups before using the result.

Zstd groups initially reserve their full decoded length only when it fits within
twice the encoded input bytes. Highly compressed groups grow from actual output;
a partial input-based reservation can overshoot when it grows. The decoded-length
check, bounded read and window limit apply to both allocation paths.
Index row columns and blueprint chunk hash/size columns are borrowed from those
decoded buffers while owned models are constructed. Length preflights bound all
column splits; no unaligned casts or borrowed data escape the decoder.

## Snapshot blueprint

Magic: `GUTCH_BLUEPRINT` followed by one zero byte. Three groups follow.

```text
1. Paths
  directory_count:u32 = D; entry_count:u32 = E
  parent_directory:u32[D+E]
  leaf_offsets:u32[D+E+1]           # first=0; final=leaf byte count
  leaf_utf8:byte[]
2. Core
  entry_kind:u8[E]                 # file=0, directory=1, symlink=2
  entry_flags:u16[E]
  file_size:u64[F]                 # file rows in entry order
  nonempty_file_hash:Hash32[H]     # zero-size files have implicit BLAKE3(empty)
  file_representation:u8[F]        # empty=0, implicit singleton=1, explicit=2
  symlink_target_entry:u32[L]
3. Explicit chunk sequences
  chunk_offsets:u32[M+1]           # consecutive sequences; first=0
  chunk_hash:Hash32[C]
  chunk_raw_size:u32[C]
```

`F`, `H`, `L` and `M` are derived from kinds, sizes and representations. `C` is
the last chunk offset. Offsets delimit adjacent spans; start/count pairs are not
stored. Each parent reference is zero for the root or a one-based directory row.
Directory rows contain shared path ancestors, sorted by full path with parents
before children. Entry rows are sorted by exact full path. Readers reject cycles,
forward directory references, duplicate/unsorted paths and invalid name spans.

Names retain exact UTF-8 bytes without Unicode normalization or global interning.
Reconstructed paths are limited to 128 KiB and 256 components. Expanded path bytes
are bounded to 256 MiB; directory and entry rows together are limited to
2,097,152. Writers and readers apply the same bounds on every platform. Symlink targets
reference existing entry rows; contained symlink cycles are not newly prohibited.

Flags use bit 0 for read-only and bit 1 for executable. Files permit both;
directories permit bit 0; symlinks permit neither. Bits 2–15 must be zero. This
portable representation is not a native mode, ACL, ownership or timestamp record.

Representation 0 requires zero size and no chunks. Representation 1 derives its
single chunk's hash and raw size from the nonempty file hash and size, which must
fit the 16 MiB raw chunk bound. Representation 2 has a nonempty explicit sequence;
chunk sizes must sum to file size. Its hashes and sizes are direct columns, not a
mandatory deduplication dictionary. File sizes remain `u64` even though individual
chunks, metadata counts and bounded metadata offsets use `u32`.

## Chunk map

Magic: `GUTCH_INDEX` followed by five zero bytes. Two groups follow.

```text
1. Artifacts
  artifact_count:u32 = A
  artifact_id:Hash32[A]            # sorted unique bundle identities
  encoded_artifact_size:u32[A]
2. Chunks
  chunk_count:u32 = N
  chunk_hash:Hash32[N]              # sorted unique binary keys
  artifact_row:u32[N]
  ordinal_or_encoded_size:u32[N]
  raw_size:u32[N]
```

Decoded group bytes total `8 + 36A + 44N`. Bundled rows store a zero-based artifact
row and physical chunk ordinal. Loose rows use `artifact_row = 0xffffffff`, store
the complete encoded file size in the next column, and use the chunk hash as
artifact identity. Artifact sizes and chunk sizes must be nonzero; raw chunks are
at most 16 MiB. References, duplicate bundle ordinals and aggregate raw bundle
sizes are validated. No timestamp or per-artifact padding is serialized, so an
equivalent mapping encodes identically regardless of regeneration time.

The runtime index retains sorted binary keys and binary search. Binary decoding
reserves the declared row count after length checks and reads the four column
slices directly into rows in wire order, without intermediate column vectors.
The index-owned bulk constructor rejects duplicate or descending keys without
sorting or dropping rows. Entries share immutable artifact identity, encoded
size and storage-mode records. Incremental updates merge sorted rows and reuse
matching artifact records. Runtime index entries contain physical artifact
references, chunk ordinals and raw chunk sizes; they do not carry proportional
encoded-size estimates. Capture derives those rounded-down estimates when loading
reuse state, using each artifact's encoded size and all indexed raw bytes for that
bundle. Loose-artifact estimates equal their complete encoded size. Raw-total and
ordinal validation remain part of index decoding.

## Bundle

```text
magic[16] = "GUTCH_BUNDLE" + zero[4]
version:u16 = 1; codec:u8; reserved:u8 = 0; chunk_count:u32 = N
payload:byte[C]                    # one encoded concatenated stream
footer:
  chunk_count:u32 = N
  chunk_hash:Hash32[N]
  chunk_raw_size:u32[N]
footer_length:u32 = 4 + 36*N
footer_magic[16] = "GUTCH_BUNDLE_END"
bundle_id:Hash32
```

Size: `C + 36N + 80` bytes. The footer locator starts at file size minus 52;
the footer starts another `footer_length` bytes earlier. BLAKE3 covers every byte
preceding the trailing binary ID, including framing and the raw columnar footer.
The object's hexadecimal filename must identify the same digest.

Raw offsets are prefix sums of sizes. The reader retains binary hashes without a
second hexadecimal copy. The footer is uncompressed; high-entropy hashes do not
justify another compression frame. Strict verification streams the payload and
checks encoded identity and raw chunk hashes. Empty bundles are supported. There
are no independent chunk frames or partial compressed payload reads.

## Loose chunk

```text
magic[16] = "GUTCH_CHUNK" + zero[5]
version:u16 = 1; codec:u8; reserved:u8 = 0
raw_size:u32; raw_hash:Hash32
payload:byte[C]
```

Size: `56 + C` bytes. Artifact codecs are none=0, Zstd=1, LZ4=2. The loose header,
requested hash/size and decoded content must agree. Encoder compression levels
are not stored. Changing capture defaults does not change an existing artifact's
codec. Loose chunks keep their compact fixed layout.

## Publication and recovery

Raw bundles are at most 256 MiB. Capture also applies the chosen preset's maximum
to complete encoded bundles and splits oversized groups at chunk boundaries.
File sizes and aggregate content totals remain `u64`; narrowing is checked.

Atomic metadata publication, repository locks, pending-generation recovery,
cleanup ordering and cancellation remain shared repository mechanisms.
`repo index rebuild` verifies physical artifacts and snapshot completeness;
`--apply` replaces the map only after successful validation. Regeneration cannot
recover missing blueprints, paths or payload bytes.

Sources: [envelope](../../src/artifact/metadata.rs),
[blueprint columns](../../src/manifest/blueprint_columns.rs),
[map codec](../../src/repo/index/binary_codec.rs),
[bundle codec](../../src/artifact/bundle.rs), [loose codec](../../src/artifact/loose.rs),
[identity](../../src/snapshot/identity.rs), [recovery](../../src/repo/recovery.rs).
