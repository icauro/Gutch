//! The repository manifest and its artifact packaging profile.

use crate::artifact::profiles::{
    ArtifactStorageMode, BundleSizingPreset, BundleSizingProfile, ChunkSizingPreset,
    ChunkSizingProfile, Chunker, CompressionKind, CompressionProfile,
};
use serde::Deserialize;
use serde::Serialize;

use super::format::{
    DocumentType, DocumentVersion, RepoFormat, RepositoryAccess, VersionedDocument,
    check_document_markers,
};
use crate::Result;
use crate::artifact::compression::validate_profile;
use crate::artifact::profiles::validate_packaging_sizes;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ArtifactProfile {
    pub chunker: Chunker,
    #[serde(rename = "chunk_size_profile")]
    pub chunk: ChunkSizingPreset,
    #[serde(rename = "bundle_size_profile")]
    pub bundle: BundleSizingPreset,
    #[serde(rename = "compression_profile")]
    pub compression: CompressionKind,
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
            chunker: Chunker::default(),
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

fn deserialize_content_packaging_profile<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<ArtifactProfile, D::Error> {
    let profile = ArtifactProfile::deserialize(deserializer)?;
    validate_packaging_sizes(
        &ChunkSizingProfile::for_preset(profile.chunk),
        &BundleSizingProfile::for_preset(profile.bundle),
        profile.storage,
    )
    .map_err(serde::de::Error::custom)?;
    validate_profile(&profile.compression_profile()).map_err(serde::de::Error::custom)?;
    Ok(profile)
}

/// The shared repository metadata document, `repo_metadata.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RepoManifest {
    pub document_type: DocumentType<RepoManifest>,
    pub document_version: DocumentVersion<RepoManifest>,
    pub repository_format: RepoFormat,
    pub repository_name: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<super::constraints::RepoConstraints>,
    #[serde(deserialize_with = "deserialize_content_packaging_profile")]
    pub content_packaging_profile: ArtifactProfile,
    /// Key slots of an encrypted repository; absent in plain repositories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption: Option<super::encryption::RepoEncryption>,
    /// Local link to a remote repository; stored outside the shared metadata.
    #[serde(skip)]
    pub remote_repo_id: Option<String>,
}

impl VersionedDocument for RepoManifest {
    const DOCUMENT_TYPE: &'static str = "gutch_repo";
    const DOCUMENT_VERSION: &'static str = "1.0.0";
}

impl RepoManifest {
    pub fn new(repository_name: String) -> Self {
        Self {
            document_type: DocumentType::default(),
            document_version: DocumentVersion::default(),
            repository_format: RepoFormat::default(),
            repository_name,
            constraints: None,
            created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            content_packaging_profile: ArtifactProfile::default(),
            encryption: None,
            remote_repo_id: None,
        }
    }

    /// Parse `repo_metadata.json` for reading. A document or repository format
    /// from another release fails before the exact parse. `source` names the
    /// document in errors.
    pub(crate) fn from_json_bytes(source: &str, bytes: &[u8]) -> Result<Self> {
        check_document_markers::<Self>(source, bytes)?;
        RepoFormat::from_repo_metadata(bytes)?.check(RepositoryAccess::Read)?;
        let manifest: Self = serde_json::from_slice(bytes)?;
        manifest.repository_format.validate()?;
        if let Some(constraints) = &manifest.constraints {
            constraints.validate_definition()?;
        }
        Ok(manifest)
    }

    pub fn set_content_packaging_profile(
        &mut self,
        chunk: ChunkSizingPreset,
        compression: CompressionKind,
        compression_level: Option<i32>,
        bundle: BundleSizingPreset,
        storage: ArtifactStorageMode,
    ) {
        self.content_packaging_profile =
            ArtifactProfile::for_presets(chunk, compression, compression_level, bundle, storage);
    }

    pub fn is_linked(&self) -> bool {
        self.remote_repo_id.is_some()
    }
}

#[cfg(test)]
#[path = "repo_manifest_tests.rs"]
mod tests;
