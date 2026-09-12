//! Native property/fuzz targets: normal cargo tests sample inputs; cargo bolero uses instrumentation.
use crate::artifact::{bundle, compression};
use crate::manifest::{CompressionKind, CompressionProfile};
use std::io::{Cursor, Read};

#[test]
fn fuzz_binary_decoders() {
    bolero::check!(
        name = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/.artifacts/fuzz/binary_decoders"
        )
    )
    .for_each(|bytes: &[u8]| {
        if bytes.len() > 65536 {
            return;
        }
        let _ = crate::manifest::binary::read_snapshot_blueprint_bytes(bytes);
        let _ = crate::repo::index::codec::load_chunk_index_binary_from_bytes(bytes);
        let _ = bundle::read_bundle(bytes, &CompressionProfile::default());
        for kind in [
            CompressionKind::None,
            CompressionKind::Lz4,
            CompressionKind::Zstd,
        ] {
            if let Ok(reader) = compression::decoder_from_reader(Cursor::new(bytes), kind) {
                let _ = reader.take(65537).read_to_end(&mut Vec::new());
            }
        }
        if let Ok(path) = std::str::from_utf8(bytes) {
            let _ = crate::filesystem::paths::validate_manifest_relative_path(path, false);
        }
    });
}

#[test]
fn fuzz_bundle_roundtrip() {
    bolero::check!(
        name = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/.artifacts/fuzz/bundle_roundtrip"
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
        let hash = crate::artifact::hash::ChunkHash::from_bytes(*blake3::hash(bytes).as_bytes());
        let mut builder = bundle::BundleBuilder::new(profile.clone());
        builder.add_chunk(hash, bytes.to_vec());
        let (id, encoded) = builder.build_with_id().unwrap();
        let mut reader = bundle::read_bundle(&encoded, &profile).unwrap();
        reader.verify_identity(&id).unwrap();
        assert_eq!(reader.read_chunk(0).unwrap(), bytes);
        let mut damaged = encoded.clone();
        let offset = bytes[0] as usize % damaged.len();
        damaged[offset] ^= 1;
        assert!(bundle::verify_bundle_identity_bytes(&id, &damaged).is_err());
    });
}
