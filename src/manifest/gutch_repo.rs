use serde::Deserialize;
use serde::Serialize;

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactProfile {
    #[serde(rename = "chunk_size_profile")]
    pub chunk: ChunkSizingPreset,
    #[serde(rename = "bundle_size_profile")]
    pub bundle: BundleSizingPreset,
    #[serde(rename = "compression_profile")]
    pub compression: CompressionKind,
    #[serde(rename = "compression_level")]
    pub compression_level: Option<i32>,
    #[serde(rename = "storage_mode")]
    pub storage: ArtifactStorageMode,
}

impl ArtifactProfile {
    pub fn for_presets(
        chunk: ChunkSizingPreset,
        compression: CompressionKind,
        compression_level: Option<i32>,
        bundle: BundleSizingPreset,
        storage: ArtifactStorageMode,
    ) -> Self {
        Self {
            chunk,
            bundle,
            compression,
            compression_level,
            storage,
        }
    }

    pub(crate) fn compression_profile(&self) -> CompressionProfile {
        CompressionProfile::for_algorithm(self.compression, self.compression_level)
    }
}

impl Default for ArtifactProfile {
    fn default() -> Self {
        Self::for_presets(
            ChunkSizingPreset::default(),
            CompressionKind::Zstd,
            Some(3),
            BundleSizingPreset::default(),
            ArtifactStorageMode::default(),
        )
    }
}

fn deserialize_artifact_profile<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<ArtifactProfile, D::Error> {
    let profile = ArtifactProfile::deserialize(deserializer)?;
    super::profiles::validate_packaging_sizes(
        &ChunkSizingProfile::for_preset(profile.chunk),
        &BundleSizingProfile::for_preset(profile.bundle),
        profile.storage,
    )
    .map_err(serde::de::Error::custom)?;
    crate::artifact::compression::validate_profile(&profile.compression_profile())
        .map_err(serde::de::Error::custom)?;
    Ok(profile)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GutchRepoManifest {
    #[serde(rename = "document_type")]
    pub format: String,
    #[serde(rename = "document_version")]
    pub format_version: String,
    #[serde(rename = "repository_name")]
    #[serde(default)]
    pub name: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<super::constraints::RepositoryConstraints>,
    #[serde(
        rename = "content_packaging_profile",
        deserialize_with = "deserialize_artifact_profile"
    )]
    pub artifact_profile: ArtifactProfile,
    #[serde(skip)]
    pub remote_repo_id: String,
}

impl GutchRepoManifest {
    pub fn new(name: String) -> Self {
        Self {
            format: CONTENT_REPO_FORMAT.to_string(),
            format_version: PUBLIC_METADATA_FORMAT_VERSION.to_string(),
            name,
            constraints: None,
            created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            artifact_profile: ArtifactProfile::default(),
            remote_repo_id: String::new(),
        }
    }

    pub fn set_artifact_profile(
        &mut self,
        chunk: ChunkSizingPreset,
        compression: CompressionKind,
        compression_level: Option<i32>,
        bundle: BundleSizingPreset,
        storage: ArtifactStorageMode,
    ) {
        self.artifact_profile =
            ArtifactProfile::for_presets(chunk, compression, compression_level, bundle, storage);
    }

    pub(crate) fn compression_profile(&self) -> CompressionProfile {
        self.artifact_profile.compression_profile()
    }

    pub fn is_linked(&self) -> bool {
        !self.remote_repo_id.is_empty()
    }

    pub fn remote_repo_id_value(&self) -> Option<i64> {
        self.remote_repo_id.parse::<i64>().ok()
    }
}

#[cfg(test)]
#[path = "gutch_repo_tests.rs"]
mod tests;
