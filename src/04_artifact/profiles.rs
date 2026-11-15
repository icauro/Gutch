//! Packaging profiles: chunker, chunk and bundle sizing, storage mode and compression.

use clap::ValueEnum;
use serde::Deserialize;
use serde::Serialize;

/// CLI names are short; serialized names pin the exact boundary algorithm.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
pub enum Chunker {
    #[default]
    #[serde(rename = "fastcdc-v2020")]
    #[value(name = "fastcdc")]
    FastCdc,
    #[serde(rename = "jc-v1.1.0")]
    #[value(name = "jc")]
    Jc,
    #[serde(rename = "ram-v1")]
    #[value(name = "vram256")]
    Vram256,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
pub enum ArtifactStorageMode {
    #[serde(rename = "bundled")]
    #[value(name = "bundled")]
    #[default]
    Bundled,
    #[serde(rename = "loose_chunks")]
    #[value(name = "loose-chunks")]
    LooseChunks,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompressionKind {
    #[serde(rename = "none")]
    #[value(name = "none")]
    None,
    #[default]
    #[serde(rename = "zstd")]
    #[value(name = "zstd")]
    Zstd,
    #[serde(rename = "lz4")]
    #[value(name = "lz4")]
    Lz4,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompressionUnit {
    #[default]
    #[serde(rename = "bundle")]
    Bundle,
}

impl ArtifactStorageMode {
    /// Name used in statistics output, matching the serialized form.
    pub(crate) fn stats_name(self) -> &'static str {
        match self {
            Self::Bundled => "bundled",
            Self::LooseChunks => "loose_chunks",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
pub enum ChunkSizingPreset {
    #[serde(rename = "chunk_512B")]
    #[value(name = "512B")]
    Chunk512B,
    #[serde(rename = "chunk_1KiB")]
    #[value(name = "1KiB")]
    Chunk1KiB,
    #[serde(rename = "chunk_2KiB")]
    #[value(name = "2KiB")]
    Chunk2KiB,
    #[serde(rename = "chunk_4KiB")]
    #[value(name = "4KiB")]
    Chunk4KiB,
    #[serde(rename = "chunk_8KiB")]
    #[value(name = "8KiB")]
    Chunk8KiB,
    #[serde(rename = "chunk_16KiB")]
    #[value(name = "16KiB")]
    Chunk16KiB,
    #[serde(rename = "chunk_32KiB")]
    #[value(name = "32KiB")]
    Chunk32KiB,
    #[serde(rename = "chunk_64KiB")]
    #[value(name = "64KiB")]
    Chunk64KiB,
    #[serde(rename = "chunk_128KiB")]
    #[value(name = "128KiB")]
    #[default]
    Chunk128KiB,
    #[serde(rename = "chunk_256KiB")]
    #[value(name = "256KiB")]
    Chunk256KiB,
    #[serde(rename = "chunk_512KiB")]
    #[value(name = "512KiB")]
    Chunk512KiB,
    #[serde(rename = "chunk_1MiB")]
    #[value(name = "1MiB")]
    Chunk1MiB,
    #[serde(rename = "chunk_2MiB")]
    #[value(name = "2MiB")]
    Chunk2MiB,
    #[serde(rename = "chunk_4MiB")]
    #[value(name = "4MiB")]
    Chunk4MiB,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
pub enum BundleSizingPreset {
    #[serde(rename = "bundle_4MiB")]
    #[value(name = "4MiB")]
    Bundle4MiB,
    #[serde(rename = "bundle_8MiB")]
    #[value(name = "8MiB")]
    #[default]
    Bundle8MiB,
    #[serde(rename = "bundle_16MiB")]
    #[value(name = "16MiB")]
    Bundle16MiB,
    #[serde(rename = "bundle_32MiB")]
    #[value(name = "32MiB")]
    Bundle32MiB,
    #[serde(rename = "bundle_64MiB")]
    #[value(name = "64MiB")]
    Bundle64MiB,
    #[serde(rename = "bundle_128MiB")]
    #[value(name = "128MiB")]
    Bundle128MiB,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChunkSizingProfile {
    pub preset: ChunkSizingPreset,
    pub chunker: Chunker,
    pub min_chunk_bytes: u32,
    pub avg_chunk_bytes: u32,
    pub max_chunk_bytes: u32,
}

impl Default for ChunkSizingProfile {
    fn default() -> Self {
        Self::for_preset(ChunkSizingPreset::Chunk128KiB)
    }
}

impl ChunkSizingProfile {
    pub fn for_preset(preset: ChunkSizingPreset) -> Self {
        let (min_chunk_bytes, avg_chunk_bytes, max_chunk_bytes) = match preset {
            ChunkSizingPreset::Chunk512B => (128, 512, 2048),
            ChunkSizingPreset::Chunk2KiB => (512, 2048, 8192),
            ChunkSizingPreset::Chunk8KiB => (2048, 8192, 32768),
            ChunkSizingPreset::Chunk2MiB => (512 * 1024, 2 * 1024 * 1024, 8 * 1024 * 1024),
            ChunkSizingPreset::Chunk4MiB => (1024 * 1024, 4 * 1024 * 1024, 16 * 1024 * 1024),
            ChunkSizingPreset::Chunk1KiB => (256, 1024, 4 * 1024),
            ChunkSizingPreset::Chunk4KiB => (1024, 4 * 1024, 16 * 1024),
            ChunkSizingPreset::Chunk16KiB => (4 * 1024, 16 * 1024, 64 * 1024),
            ChunkSizingPreset::Chunk32KiB => (8 * 1024, 32 * 1024, 128 * 1024),
            ChunkSizingPreset::Chunk64KiB => (16 * 1024, 64 * 1024, 256 * 1024),
            ChunkSizingPreset::Chunk128KiB => (32 * 1024, 128 * 1024, 512 * 1024),
            ChunkSizingPreset::Chunk256KiB => (64 * 1024, 256 * 1024, 1024 * 1024),
            ChunkSizingPreset::Chunk512KiB => (128 * 1024, 512 * 1024, 2 * 1024 * 1024),
            ChunkSizingPreset::Chunk1MiB => (256 * 1024, 1024 * 1024, 4 * 1024 * 1024),
        };

        ChunkSizingProfile {
            preset,
            chunker: Chunker::FastCdc,
            min_chunk_bytes,
            avg_chunk_bytes,
            max_chunk_bytes,
        }
    }

    pub fn with_chunker(mut self, chunker: Chunker) -> Self {
        self.chunker = chunker;
        self.min_chunk_bytes = if chunker == Chunker::Vram256 {
            self.avg_chunk_bytes
        } else {
            self.avg_chunk_bytes / 4
        };
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BundleSizingProfile {
    pub preset: BundleSizingPreset,
    pub bundle_max_mib: u64,
    pub bundle_preferred_mib: u64,
}

impl BundleSizingProfile {
    pub fn for_preset(preset: BundleSizingPreset) -> Self {
        let (bundle_max_mib, bundle_preferred_mib) = match preset {
            BundleSizingPreset::Bundle16MiB => (32, 16),
            BundleSizingPreset::Bundle4MiB => (8, 4),
            BundleSizingPreset::Bundle8MiB => (16, 8),
            BundleSizingPreset::Bundle32MiB => (64, 32),
            BundleSizingPreset::Bundle64MiB => (128, 64),
            BundleSizingPreset::Bundle128MiB => (256, 128),
        };

        BundleSizingProfile {
            preset,
            bundle_max_mib,
            bundle_preferred_mib,
        }
    }

    pub fn bundle_max_size_bytes(&self) -> u64 {
        self.bundle_max_mib * 1024 * 1024
    }

    pub fn bundle_preferred_size_bytes(&self) -> u64 {
        self.bundle_preferred_mib * 1024 * 1024
    }
}

impl Default for BundleSizingProfile {
    fn default() -> Self {
        Self::for_preset(BundleSizingPreset::Bundle8MiB)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CompressionProfile {
    pub compression: CompressionKind,
    pub compression_unit: CompressionUnit,
    pub level: Option<i32>,
}

impl CompressionProfile {
    pub fn for_algorithm(compression: CompressionKind, level: Option<i32>) -> Self {
        CompressionProfile {
            compression,
            compression_unit: CompressionUnit::Bundle,
            level,
        }
    }
}

impl Default for CompressionProfile {
    fn default() -> Self {
        Self::for_algorithm(CompressionKind::Zstd, Some(3))
    }
}

/// Reject combinations that cannot accommodate a maximum-size raw chunk with
/// codec/framing overhead. The packing stage still enforces the encoded cap.
pub(crate) fn validate_packaging_sizes(
    chunk: &ChunkSizingProfile,
    bundle: &BundleSizingProfile,
    storage: ArtifactStorageMode,
) -> crate::Result<()> {
    if storage == ArtifactStorageMode::Bundled
        && u64::from(chunk.max_chunk_bytes) >= bundle.bundle_max_size_bytes()
    {
        return Err(crate::Error::invalid_input(
            "bundle maximum must exceed the maximum raw chunk size; choose a larger bundle preset or loose-chunks storage",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod sizing_tests {
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
}

#[cfg(test)]
mod contract_tests {
    use super::*;
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
}
