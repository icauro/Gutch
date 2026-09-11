# Gutch Binary File Layouts

Status: Current

Scope: Artifact, chunk-index, and snapshot-blueprint bytes

Verified: 2026-09-11

These are serialized layouts, without Rust alignment padding. Integers are
unsigned little-endian. `Hash32` is binary BLAKE3; `Hex64` is its lowercase
ASCII encoding. `String` means `{length: u32, utf8: byte[length]}`. Lengths
count bytes. Every top-level binary format uses version **1**, a 16-byte magic,
and exact zero padding. Older formats are rejected, even if they also used a
version numbered 1. JSON documents retain `document_version: "1.0.0"`.

## Files and roles

| Repository path | Role |
|---|---|
| `snapshots/<id>/snapshot_blueprint.gtcbin` | Tree entries and ordered chunk references |
| `chunk_map.gtcbin` | Sorted chunk hashes mapped to artifact locations |
| `bundles/<prefix>/<id>` | One compressed stream and its physical chunk table |
| `chunks/<prefix>/<hash>` | One self-describing loose chunk |
| `repo_metadata.json` | Capture defaults; existing artifacts supply their own codec |

A blueprint describes logical file order; an index describes the selected
location for each hash; a bundle table describes its physical contents. Keeping
the latter allows inspection and index regeneration without the old index.

## Bundle

```text
Bundle {
    magic: byte[16] = "GUTCH_BUNDLE" + zero[4]
    version: u16 = 1
    codec: u8                 // 0 none, 1 zstd, 2 lz4
    reserved: u8 = 0
    chunk_count: u32 = N
    payload: byte[C]          // one encoded concatenated chunk stream
    footer: {
        chunk_count: u32 = N
        chunks: { hash: Hash32, raw_size: u32 }[N]
    }
    footer_length: u32 = 4 + 36*N
    footer_magic: byte[16] = "GUTCH_BUNDLE_END"
    bundle_id: Hex64
}
```

**Size: `C + 36N + 112` bytes.** Header: 24 bytes; each chunk table row:
36 bytes; final locator plus ID: 84 bytes. From file size `S`, footer length
starts at `S - 84`; the footer starts at `S - 84 - footer_length`.
BLAKE3 covers every byte preceding the trailing ID. The filename must match
that ID. Each chunk hash independently covers its raw bytes.

Raw offsets are prefix sums of chunk sizes. Compressed offsets are absent:
chunks share one stream. Runtime compressed-size estimates are accounting
values, never integrity evidence. Readers stream the complete payload and fan
out needed chunks; strict scrubbing hashes encoded bytes and raw chunks in one
payload pass. Empty bundles have `N = 0`; compressed empty frames can make
`C > 0`. No independent chunk frames or partial compressed reads are provided.

## Loose chunk

```text
LooseChunk {
    magic: byte[16] = "GUTCH_CHUNK" + zero[5]
    version: u16 = 1
    codec: u8
    reserved: u8 = 0
    raw_size: u32
    raw_hash: Hash32
    payload: byte[C]
}
```

**Size: `56 + C` bytes.** For `none`, `C = raw_size`. The path hash, header
hash, decoded size, and raw content hash must agree. Compression level is only
an encoder setting; readers need the codec, not the original level or repo
profile. Both artifact kinds support `none`, `zstd`, and `lz4`.

## Chunk map

```text
ChunkIndex {
    magic: byte[16] = "GUTCH_INDEX" + zero[5]
    version: u16 = 1
    flags: u16 = 0
    chunk_count: u64 = N
    artifact_count: u32 = A
    updated_at: String
    artifacts: {
        id: Hash32
        encoded_file_size: u32
        kind: u8 = 0          // bundle
        reserved: byte[3] = zero[3]
    }[A]                      // 40 bytes per distinct bundle
    chunks: {
        hash: Hash32
        artifact_row: u32
        ordinal_or_size: u32
        raw_size: u32
    }[N]                      // 44 bytes, sorted by binary hash
}
```

**Size: `36 + timestamp_bytes + 40A + 44N`.** Bundled rows store the
zero-based dictionary row and chunk ordinal. Loose rows use `artifact_row =
0xffffffff`, put the complete encoded file size in `ordinal_or_size`, and use
the chunk hash as artifact identity; they need no dictionary entry.

Readers reject duplicate/unsorted chunk hashes, duplicate dictionary IDs or
bundle ordinals, invalid dictionary references, zero/oversized chunk sizes,
nonzero reserved fields, truncated counts, and trailing bytes. The in-memory
index is a sorted vector keyed by binary hashes. Long bundle IDs are shared
across entries; incremental capture merges sorted updates in batches.

At roughly 256 chunks per bundle, bundle dictionary overhead is about
`40/256 = 0.156` bytes per indexed chunk. The bundle table plus index adds
about 80 bytes per unique bundled chunk before fixed framing and blueprints:
about 0.061% of a 128 KiB raw chunk. Filesystem allocation and compression
ratios can dominate these metadata savings.

## Snapshot blueprint

```text
Blueprint {
    magic: byte[16] = "GUTCH_BLUEPRINT" + zero[1]
    version: u16 = 1
    reserved: u16 = 0
    entry_count: u32
    total_chunk_references: u64
    entries: Entry[entry_count]
}
FileEntry {                   // kind 0
    kind: u8 = 0
    path: String
    file_size: u64
    executable: u8            // 0 or 1
    read_only: u8             // 0 or 1
    full_file_hash: Hash32
    chunk_count: u32 = R
    chunks: { hash: Hash32, raw_size: u32 }[R]
}
DirectoryEntry { kind: u8 = 1, path: String, read_only: u8 }
SymlinkEntry   { kind: u8 = 2, path: String, target: String }
```

Header: 32 bytes. File: `51 + path_bytes + 36R`; directory:
`6 + path_bytes`; symlink: `9 + path_bytes + target_bytes`. Chunk sizes must
sum exactly to file size, including empty files. Paths and symlink targets
also undergo installation safety validation. Installation identity retains its
existing content/property encoding; changing packaging does not change it.

## Bounds, publication, and recovery

Chunks are nonempty and at most 4 MiB. Raw bundles are at most 256 MiB.
Capture also enforces the chosen preset's maximum against the complete encoded
bundle, splitting oversized groups at chunk boundaries. The default is
32/128/512 KiB chunk minimum/average/maximum and 8 MiB preferred / 16 MiB
maximum encoded bundles. `bundle_32MiB` means 32 MiB preferred / 64 MiB maximum.
Sizes and bundle-local offsets use `u32`; file offsets, file sizes, and totals
remain `u64`. Arithmetic is checked before narrowing.

Mutations use an exclusive `.gutch-lock`; cooperating readers use shared locks.
`.gutch-transactions/committed` names the saved metadata generation; `pending`
marks interrupted publication. The next mutation restores that generation
before proceeding. Readers reject pending publication explicitly. Immutable
artifact cleanup follows metadata commit. These local process-crash protections
do not provide remote transactions or a tested power-loss guarantee.

`repo regenerate-chunk-map --repo <path>` verifies bundles and loose chunks and
reports completeness without replacing the map. Add `--publish` to replace it
under exclusive ownership. Every retained snapshot must resolve; corruption or
missing content prevents publication. This cannot recover lost blueprints,
original paths, or missing payload bytes.

Sources: [formats](../../src/artifact/format.rs),
[bundles](../../src/artifact/bundle.rs), [loose chunks](../../src/artifact/store.rs),
[binary metadata](../../src/manifest/binary.rs),
[coordination](../../src/repo/coordination.rs), [recovery](../../src/repo/recovery.rs).
