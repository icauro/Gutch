use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use super::{CHUNK_HASH_BLAKE3, CHUNKER_FASTCDC, FORMAT_VERSION};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
pub enum ArtifactStorageMode {
    #[serde(rename = "bundled")]
    #[value(name = "bundled")]
    #[default]
    Bundled,
    #[serde(rename = "loose_chunks")]
    #[value(name = "loose_chunks")]
    LooseChunks,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompressionKind {
    #[serde(rename = "none")]
    None,
    #[default]
    #[serde(rename = "zstd")]
    Zstd,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompressionUnit {
    #[default]
    #[serde(rename = "bundle")]
    Bundle,
}

impl ArtifactStorageMode {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Bundled => "bundled",
            Self::LooseChunks => "loose_chunks",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
pub enum ArtifactSizingPreset {
    #[serde(rename = "chunk_1KiB")]
    #[value(name = "chunk_1KiB")]
    Chunk1KiB,
    #[serde(rename = "chunk_4KiB")]
    #[value(name = "chunk_4KiB")]
    Chunk4KiB,
    #[serde(rename = "chunk_16KiB")]
    #[value(name = "chunk_16KiB")]
    Chunk16KiB,
    #[serde(rename = "chunk_32KiB")]
    #[value(name = "chunk_32KiB")]
    Chunk32KiB,
    #[serde(rename = "chunk_64KiB")]
    #[value(name = "chunk_64KiB")]
    Chunk64KiB,
    #[serde(rename = "chunk_128KiB")]
    #[value(name = "chunk_128KiB")]
    #[default]
    Chunk128KiB,
    #[serde(rename = "chunk_256KiB")]
    #[value(name = "chunk_256KiB")]
    Chunk256KiB,
    #[serde(rename = "chunk_512KiB")]
    #[value(name = "chunk_512KiB")]
    Chunk512KiB,
    #[serde(rename = "chunk_1MiB")]
    #[value(name = "chunk_1MiB")]
    Chunk1MiB,
}

impl ArtifactSizingPreset {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Chunk1KiB => "chunk_1KiB",
            Self::Chunk4KiB => "chunk_4KiB",
            Self::Chunk16KiB => "chunk_16KiB",
            Self::Chunk32KiB => "chunk_32KiB",
            Self::Chunk64KiB => "chunk_64KiB",
            Self::Chunk128KiB => "chunk_128KiB",
            Self::Chunk256KiB => "chunk_256KiB",
            Self::Chunk512KiB => "chunk_512KiB",
            Self::Chunk1MiB => "chunk_1MiB",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
pub enum BundleSizingPreset {
    #[serde(rename = "bundle_4MiB")]
    #[value(name = "bundle_4MiB")]
    Bundle4MiB,
    #[serde(rename = "bundle_8MiB")]
    #[value(name = "bundle_8MiB")]
    Bundle8MiB,
    #[serde(rename = "bundle_16MiB")]
    #[value(name = "bundle_16MiB")]
    #[default]
    Bundle16MiB,
    #[serde(rename = "bundle_32MiB")]
    #[value(name = "bundle_32MiB")]
    Bundle32MiB,
    #[serde(rename = "bundle_64MiB")]
    #[value(name = "bundle_64MiB")]
    Bundle64MiB,
    #[serde(rename = "bundle_128MiB")]
    #[value(name = "bundle_128MiB")]
    Bundle128MiB,
}

impl BundleSizingPreset {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Bundle4MiB => "bundle_4MiB",
            Self::Bundle8MiB => "bundle_8MiB",
            Self::Bundle16MiB => "bundle_16MiB",
            Self::Bundle32MiB => "bundle_32MiB",
            Self::Bundle64MiB => "bundle_64MiB",
            Self::Bundle128MiB => "bundle_128MiB",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChunkSizingProfile {
    pub preset: ArtifactSizingPreset,
    pub chunker: SmolStr,
    pub min_chunk_bytes: u32,
    pub avg_chunk_bytes: u32,
    pub max_chunk_bytes: u32,
    pub gear_table_id: u32,
    pub chunk_hash_algorithm: SmolStr,
    pub format_version: SmolStr,
}

impl Default for ChunkSizingProfile {
    fn default() -> Self {
        Self::for_preset(ArtifactSizingPreset::Chunk128KiB)
    }
}

impl ChunkSizingProfile {
    pub fn for_preset(preset: ArtifactSizingPreset) -> Self {
        let (min_chunk_bytes, avg_chunk_bytes, max_chunk_bytes) = match preset {
            ArtifactSizingPreset::Chunk1KiB => (256, 1024, 4 * 1024),
            ArtifactSizingPreset::Chunk4KiB => (1024, 4 * 1024, 16 * 1024),
            ArtifactSizingPreset::Chunk16KiB => (4 * 1024, 16 * 1024, 64 * 1024),
            ArtifactSizingPreset::Chunk32KiB => (8 * 1024, 32 * 1024, 128 * 1024),
            ArtifactSizingPreset::Chunk64KiB => (16 * 1024, 64 * 1024, 256 * 1024),
            ArtifactSizingPreset::Chunk128KiB => (32 * 1024, 128 * 1024, 512 * 1024),
            ArtifactSizingPreset::Chunk256KiB => (64 * 1024, 256 * 1024, 1024 * 1024),
            ArtifactSizingPreset::Chunk512KiB => (128 * 1024, 512 * 1024, 2 * 1024 * 1024),
            ArtifactSizingPreset::Chunk1MiB => (256 * 1024, 1024 * 1024, 4 * 1024 * 1024),
        };

        ChunkSizingProfile {
            preset,
            chunker: CHUNKER_FASTCDC.into(),
            min_chunk_bytes,
            avg_chunk_bytes,
            max_chunk_bytes,
            gear_table_id: 3,
            chunk_hash_algorithm: CHUNK_HASH_BLAKE3.into(),
            format_version: FORMAT_VERSION.into(),
        }
    }

    pub fn min_chunk_size_bytes(&self) -> u32 {
        self.min_chunk_bytes
    }

    pub fn avg_chunk_size_bytes(&self) -> u32 {
        self.avg_chunk_bytes
    }

    pub fn max_chunk_size_bytes(&self) -> u32 {
        self.max_chunk_bytes
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
        Self::for_preset(BundleSizingPreset::Bundle16MiB)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq, Default)]
pub enum CompressionPreset {
    #[serde(rename = "uncompressed")]
    #[value(name = "uncompressed")]
    Uncompressed,
    #[serde(rename = "zstd_1")]
    #[value(name = "zstd_1")]
    Zstd1,
    #[serde(rename = "zstd_3")]
    #[value(name = "zstd_3")]
    #[default]
    Zstd3,
    #[serde(rename = "zstd_5")]
    #[value(name = "zstd_5")]
    Zstd5,
}

impl CompressionPreset {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Uncompressed => "uncompressed",
            Self::Zstd1 => "zstd_1",
            Self::Zstd3 => "zstd_3",
            Self::Zstd5 => "zstd_5",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CompressionProfile {
    pub preset: CompressionPreset,
    pub compression: CompressionKind,
    pub compression_unit: CompressionUnit,
    pub zstd_level: i32,
}

impl CompressionProfile {
    pub fn for_preset(preset: CompressionPreset) -> Self {
        let (compression, zstd_level) = match preset {
            CompressionPreset::Uncompressed => (CompressionKind::None, 0),
            CompressionPreset::Zstd1 => (CompressionKind::Zstd, 1),
            CompressionPreset::Zstd3 => (CompressionKind::Zstd, 3),
            CompressionPreset::Zstd5 => (CompressionKind::Zstd, 5),
        };
        CompressionProfile {
            preset,
            compression,
            compression_unit: CompressionUnit::Bundle,
            zstd_level,
        }
    }
}

impl Default for CompressionProfile {
    fn default() -> Self {
        Self::for_preset(CompressionPreset::Zstd3)
    }
}
