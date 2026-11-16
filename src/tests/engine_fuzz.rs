//! Native property/fuzz targets: normal cargo tests sample inputs; cargo bolero uses instrumentation.
use crate::artifact::format::CHUNK_INDEX_MAGIC;
use crate::artifact::format::SNAPSHOT_BLUEPRINT_MAGIC;
use crate::artifact::hash::ChunkHash;
use crate::artifact::loose::read_loose_header;
use crate::artifact::metadata;
use crate::artifact::profiles::{CompressionKind, CompressionProfile};
use crate::artifact::{bundle, compression};
use crate::filesystem::paths::validate_manifest_relative_path;
use crate::manifest::blueprint_codec::read_snapshot_blueprint_bytes;
use crate::manifest::blueprint_columns;
use crate::repo::index::binary_codec::load_chunk_index_binary_from_bytes;
use crate::snapshot::identity::snapshot_id_from_entries;
use std::io::{Cursor, Read};
use std::str::from_utf8;

#[test]
fn fuzz_binary_decoders() {
    bolero::check!(
        name = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.artifacts/tests/fuzz/binary_decoders"
        )
    )
    .for_each(|bytes: &[u8]| {
        // Matches MAX_INPUT_BYTES in tests/helpers/fuzz.py, which admits the
        // corpus's smaller bundles as seeds.
        if bytes.len() > 131072 {
            return;
        }
        let _ = read_snapshot_blueprint_bytes(bytes);
        let _ = load_chunk_index_binary_from_bytes(bytes);
        // Recompute framing so mutations reach the column parsers instead of
        // stopping at the envelope's integrity check on every generated input.
        let split = bytes.len() / 3;
        let groups = vec![
            bytes[..split].to_vec(),
            bytes[split..split * 2].to_vec(),
            bytes[split * 2..].to_vec(),
        ];
        let _ = blueprint_columns::decode(&groups);
        if let Ok(encoded) = metadata::encode(CHUNK_INDEX_MAGIC, &groups[..2]) {
            let _ = load_chunk_index_binary_from_bytes(&encoded);
        }
        if let Ok(mut reader) = bundle::read_bundle(bytes) {
            // Opening checks only framing; streaming reaches the codec decoders.
            let required = vec![true; reader.entries().len()];
            let _ = reader.stream_bundle_chunks(None, &required, |_, _| Ok(()));
        }
        let _ = read_loose_header(&mut Cursor::new(bytes));
        for kind in [
            CompressionKind::None,
            CompressionKind::Lz4,
            CompressionKind::Zstd,
        ] {
            if let Ok(reader) = compression::decoder_from_reader(Cursor::new(bytes), kind) {
                let _ = reader.take(65537).read_to_end(&mut Vec::new());
            }
        }
        if let Ok(path) = from_utf8(bytes) {
            let _ = validate_manifest_relative_path(path, false);
        }
    });
}

#[test]
fn fuzz_bundle_roundtrip() {
    bolero::check!(
        name = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.artifacts/tests/fuzz/bundle_roundtrip"
        )
    )
    .for_each(|bytes: &[u8]| {
        if bytes.is_empty() || bytes.len() > 65536 {
            return;
        }
        let kind = [
            CompressionKind::None,
            CompressionKind::Lz4,
            CompressionKind::Zstd,
        ][bytes[0] as usize % 3];
        let profile = CompressionProfile {
            compression: kind,
            level: None,
            ..Default::default()
        };
        let hash = ChunkHash::from_bytes(*blake3::hash(bytes).as_bytes());
        let mut builder = bundle::BundleBuilder::new(profile.clone());
        builder.add_chunk(hash, bytes.to_vec());
        let (id, encoded) = builder.build_with_id().unwrap();
        let mut reader = bundle::read_bundle(&encoded).unwrap();
        reader.verify_identity(&id).unwrap();
        assert_eq!(reader.read_chunk(0).unwrap(), bytes);
        let mut damaged = encoded.clone();
        let offset = bytes[0] as usize % damaged.len();
        damaged[offset] ^= 1;
        assert!(bundle::verify_bundle_identity_bytes(&id, &damaged).is_err());
    });
}

#[test]
fn fuzz_blueprint_roundtrip() {
    bolero::check!(
        name = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.artifacts/tests/fuzz/blueprint_roundtrip"
        )
    )
    .for_each(|bytes: &[u8]| {
        if bytes.len() > 4096 {
            return;
        }
        use crate::manifest::{
            ChunkRef, DirectoryManifest, FileManifest, SnapshotEntry, SymlinkManifest,
        };
        let properties = bytes.first().copied().unwrap_or(0);
        let chunks = bytes
            .chunks(1 + usize::from(properties))
            .map(|chunk| ChunkRef {
                chunk_hash: ChunkHash::of(chunk),
                raw_size: chunk.len() as u32,
            })
            .collect();
        let entries = vec![
            SnapshotEntry::Directory(DirectoryManifest {
                path: "目录".into(),
                read_only: properties & 1 != 0,
            }),
            SnapshotEntry::File(FileManifest {
                path: "目录/λ🙂".into(),
                size: bytes.len() as u64,
                executable: properties & 2 != 0,
                read_only: properties & 4 != 0,
                full_file_hash: ChunkHash::of(bytes),
                chunks,
            }),
            SnapshotEntry::Symlink(SymlinkManifest {
                path: "link".into(),
                target_path: "目录/λ🙂".into(),
            }),
        ];
        let groups = blueprint_columns::encode(&entries).unwrap();
        let encoded = metadata::encode(SNAPSHOT_BLUEPRINT_MAGIC, &groups).unwrap();
        let restored = read_snapshot_blueprint_bytes(&encoded).unwrap();
        assert_eq!(
            snapshot_id_from_entries(&entries),
            snapshot_id_from_entries(&restored)
        );
        assert_eq!(blueprint_columns::encode(&restored).unwrap(), groups);
        let mut mutated = groups;
        let group = usize::from(properties) % mutated.len();
        if !mutated[group].is_empty() {
            let offset = bytes.len() % mutated[group].len();
            mutated[group][offset] ^= 1;
            let _ = blueprint_columns::decode(&mutated);
        }
    });
}
