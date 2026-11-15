use super::*;

#[test]
fn expanded_presets_chunk_real_input_and_preserve_short_files() {
    let mut state = 0x12345678u64;
    let data = (0..20 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect::<Vec<_>>();
    for preset in ChunkSizingPreset::value_variants() {
        let profile = ChunkSizingProfile::for_preset(*preset);
        let chunks = crate::artifact::chunk::chunk_bytes(&data, &profile);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.size).sum::<u64>(),
            data.len() as u64
        );
        let mut offset = 0;
        for chunk in chunks {
            assert_eq!(chunk.offset, offset);
            assert!(chunk.size > 0 && chunk.size <= u64::from(profile.max_chunk_bytes));
            assert_eq!(
                chunk.hash,
                crate::artifact::hash::ChunkHash::of(
                    &data[offset as usize..(offset + chunk.size) as usize]
                )
            );
            offset += chunk.size;
        }
        assert!(crate::artifact::chunk::chunk_bytes(&[], &profile).is_empty());
        let short = crate::artifact::chunk::chunk_bytes(b"short", &profile);
        assert_eq!(short.len(), 1);
        assert_eq!(short[0].size, 5);
    }
}

#[test]
fn chunk_presets_cover_every_power_of_two() {
    let presets = ChunkSizingPreset::value_variants();
    assert_eq!(presets.len(), 14);
    let mut averages = presets
        .iter()
        .map(|preset| {
            let profile = ChunkSizingProfile::for_preset(*preset);
            assert_eq!(profile.min_chunk_bytes, profile.avg_chunk_bytes / 4);
            assert_eq!(profile.max_chunk_bytes, profile.avg_chunk_bytes * 4);
            let encoded = serde_json::to_string(preset).unwrap();
            assert_eq!(*preset, serde_json::from_str(&encoded).unwrap());
            profile.avg_chunk_bytes
        })
        .collect::<Vec<_>>();
    averages.sort();
    assert_eq!(
        averages,
        (9..=22).map(|power| 1u32 << power).collect::<Vec<_>>()
    );
}
#[test]
fn oversized_chunks_require_a_larger_bundle_or_loose_storage() {
    let chunk = ChunkSizingProfile::for_preset(ChunkSizingPreset::Chunk4MiB);
    for (preset, valid) in [
        (BundleSizingPreset::Bundle4MiB, false),
        (BundleSizingPreset::Bundle8MiB, false),
        (BundleSizingPreset::Bundle16MiB, true),
    ] {
        let bundle = BundleSizingProfile::for_preset(preset);
        assert_eq!(
            validate_packaging_sizes(&chunk, &bundle, ArtifactStorageMode::Bundled).is_ok(),
            valid
        );
        assert!(
            validate_packaging_sizes(&chunk, &bundle, ArtifactStorageMode::LooseChunks).is_ok()
        );
    }
}

#[test]
fn chunking_profile_defaults() {
    let p = ChunkSizingProfile::default();
    assert_eq!(p.preset, ChunkSizingPreset::Chunk128KiB);
    assert_eq!(p.min_chunk_bytes, 32 * 1024);
    assert_eq!(p.avg_chunk_bytes, 128 * 1024);
    assert_eq!(p.max_chunk_bytes, 512 * 1024);
    assert_eq!(p.min_chunk_bytes, 32 * 1024);
    assert_eq!(p.avg_chunk_bytes, 128 * 1024);
    assert_eq!(p.max_chunk_bytes, 512 * 1024);
}

#[test]
fn small_chunk_profile_uses_sub_kib_range() {
    let p = ChunkSizingProfile::for_preset(ChunkSizingPreset::Chunk1KiB);
    assert_eq!(p.preset, ChunkSizingPreset::Chunk1KiB);
    assert_eq!(p.min_chunk_bytes, 256);
    assert_eq!(p.avg_chunk_bytes, 1024);
    assert_eq!(p.max_chunk_bytes, 4 * 1024);
}

#[test]
fn chunk_profile_4kib_uses_expected_range() {
    let p = ChunkSizingProfile::for_preset(ChunkSizingPreset::Chunk4KiB);
    assert_eq!(p.preset, ChunkSizingPreset::Chunk4KiB);
    assert_eq!(p.min_chunk_bytes, 1024);
    assert_eq!(p.avg_chunk_bytes, 4 * 1024);
    assert_eq!(p.max_chunk_bytes, 16 * 1024);
}

#[test]
fn chunk_profile_512kib_uses_expected_range() {
    let p = ChunkSizingProfile::for_preset(ChunkSizingPreset::Chunk512KiB);
    assert_eq!(p.preset, ChunkSizingPreset::Chunk512KiB);
    assert_eq!(p.min_chunk_bytes, 128 * 1024);
    assert_eq!(p.avg_chunk_bytes, 512 * 1024);
    assert_eq!(p.max_chunk_bytes, 2 * 1024 * 1024);
}

#[test]
fn chunk_profile_1mib_uses_expected_range() {
    let p = ChunkSizingProfile::for_preset(ChunkSizingPreset::Chunk1MiB);
    assert_eq!(p.preset, ChunkSizingPreset::Chunk1MiB);
    assert_eq!(p.min_chunk_bytes, 256 * 1024);
    assert_eq!(p.avg_chunk_bytes, 1024 * 1024);
    assert_eq!(p.max_chunk_bytes, 4 * 1024 * 1024);
}

#[test]
fn bundle_sizing_profile_defaults() {
    let p = BundleSizingProfile::default();
    assert_eq!(p.preset, BundleSizingPreset::Bundle8MiB);
    assert_eq!(p.bundle_max_mib, 16);
    assert_eq!(p.bundle_preferred_mib, 8);
    assert_eq!(p.bundle_max_size_bytes(), 16 * 1024 * 1024);
    assert_eq!(p.bundle_preferred_size_bytes(), 8 * 1024 * 1024);
}

#[test]
fn compression_profile_defaults() {
    let p = CompressionProfile::default();
    assert_eq!(p.level, Some(3));
    assert_eq!(p.compression, CompressionKind::Zstd);
    assert_eq!(p.compression_unit, CompressionUnit::Bundle);
}

#[test]
fn compression_profile_zstd5_level() {
    let p = CompressionProfile::for_algorithm(CompressionKind::Zstd, Some(5));
    assert_eq!(p.level, Some(5));
    assert_eq!(p.compression, CompressionKind::Zstd);
    assert_eq!(p.compression_unit, CompressionUnit::Bundle);
}
