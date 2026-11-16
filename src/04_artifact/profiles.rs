//! Packaging profiles: chunker, chunk and bundle sizing, storage mode and compression.

use serde::Deserialize;
use serde::Serialize;

/// CLI names are short; serialized names pin the exact boundary algorithm.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub enum Chunker {
    #[default]
    #[serde(rename = "fastcdc-v2020")]
    #[cfg_attr(feature = "cli", value(name = "fastcdc"))]
    FastCdc,
    #[serde(rename = "jc-v1.1.0")]
    #[cfg_attr(feature = "cli", value(name = "jc"))]
    Jc,
    #[serde(rename = "ram-v1")]
    #[cfg_attr(feature = "cli", value(name = "vram256"))]
    Vram256,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub enum ArtifactStorageMode {
    #[serde(rename = "bundled")]
    #[cfg_attr(feature = "cli", value(name = "bundled"))]
    #[default]
    Bundled,
    #[serde(rename = "loose_chunks")]
    #[cfg_attr(feature = "cli", value(name = "loose-chunks"))]
    LooseChunks,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CompressionKind {
    #[serde(rename = "none")]
    #[cfg_attr(feature = "cli", value(name = "none"))]
    None,
    #[default]
    #[serde(rename = "zstd")]
    #[cfg_attr(feature = "cli", value(name = "zstd"))]
    Zstd,
    #[serde(rename = "lz4")]
    #[cfg_attr(feature = "cli", value(name = "lz4"))]
    Lz4,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub enum ChunkSizingPreset {
    #[serde(rename = "chunk_512B")]
    #[cfg_attr(feature = "cli", value(name = "512B"))]
    Chunk512B,
    #[serde(rename = "chunk_1KiB")]
    #[cfg_attr(feature = "cli", value(name = "1KiB"))]
    Chunk1KiB,
    #[serde(rename = "chunk_2KiB")]
    #[cfg_attr(feature = "cli", value(name = "2KiB"))]
    Chunk2KiB,
    #[serde(rename = "chunk_4KiB")]
    #[cfg_attr(feature = "cli", value(name = "4KiB"))]
    Chunk4KiB,
    #[serde(rename = "chunk_8KiB")]
    #[cfg_attr(feature = "cli", value(name = "8KiB"))]
    Chunk8KiB,
    #[serde(rename = "chunk_16KiB")]
    #[cfg_attr(feature = "cli", value(name = "16KiB"))]
    Chunk16KiB,
    #[serde(rename = "chunk_32KiB")]
    #[cfg_attr(feature = "cli", value(name = "32KiB"))]
    Chunk32KiB,
    #[serde(rename = "chunk_64KiB")]
    #[cfg_attr(feature = "cli", value(name = "64KiB"))]
    Chunk64KiB,
    #[serde(rename = "chunk_128KiB")]
    #[cfg_attr(feature = "cli", value(name = "128KiB"))]
    #[default]
    Chunk128KiB,
    #[serde(rename = "chunk_256KiB")]
    #[cfg_attr(feature = "cli", value(name = "256KiB"))]
    Chunk256KiB,
    #[serde(rename = "chunk_512KiB")]
    #[cfg_attr(feature = "cli", value(name = "512KiB"))]
    Chunk512KiB,
    #[serde(rename = "chunk_1MiB")]
    #[cfg_attr(feature = "cli", value(name = "1MiB"))]
    Chunk1MiB,
    #[serde(rename = "chunk_2MiB")]
    #[cfg_attr(feature = "cli", value(name = "2MiB"))]
    Chunk2MiB,
    #[serde(rename = "chunk_4MiB")]
    #[cfg_attr(feature = "cli", value(name = "4MiB"))]
    Chunk4MiB,
}

impl ChunkSizingPreset {
    /// Every preset, smallest first.
    pub const ALL: [Self; 14] = [
        Self::Chunk512B,
        Self::Chunk1KiB,
        Self::Chunk2KiB,
        Self::Chunk4KiB,
        Self::Chunk8KiB,
        Self::Chunk16KiB,
        Self::Chunk32KiB,
        Self::Chunk64KiB,
        Self::Chunk128KiB,
        Self::Chunk256KiB,
        Self::Chunk512KiB,
        Self::Chunk1MiB,
        Self::Chunk2MiB,
        Self::Chunk4MiB,
    ];
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub enum BundleSizingPreset {
    #[serde(rename = "bundle_4MiB")]
    #[cfg_attr(feature = "cli", value(name = "4MiB"))]
    Bundle4MiB,
    #[serde(rename = "bundle_8MiB")]
    #[cfg_attr(feature = "cli", value(name = "8MiB"))]
    #[default]
    Bundle8MiB,
    #[serde(rename = "bundle_16MiB")]
    #[cfg_attr(feature = "cli", value(name = "16MiB"))]
    Bundle16MiB,
    #[serde(rename = "bundle_32MiB")]
    #[cfg_attr(feature = "cli", value(name = "32MiB"))]
    Bundle32MiB,
    #[serde(rename = "bundle_64MiB")]
    #[cfg_attr(feature = "cli", value(name = "64MiB"))]
    Bundle64MiB,
    #[serde(rename = "bundle_128MiB")]
    #[cfg_attr(feature = "cli", value(name = "128MiB"))]
    Bundle128MiB,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
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
#[cfg_attr(test, derive(schemars::JsonSchema))]
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
#[cfg_attr(test, derive(schemars::JsonSchema))]
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
#[path = "profiles_tests.rs"]
mod tests;
