use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactProfile {
    #[serde(rename = "chunk_size_profile")]
    pub chunk: ArtifactSizingPreset,
    #[serde(rename = "bundle_size_profile")]
    pub bundle: BundleSizingPreset,
    #[serde(rename = "compression_profile")]
    pub compression: CompressionPreset,
    #[serde(rename = "storage_mode")]
    pub storage: ArtifactStorageMode,
}

impl Default for ArtifactProfile {
    fn default() -> Self {
        Self {
            chunk: ArtifactSizingPreset::default(),
            bundle: BundleSizingPreset::default(),
            compression: CompressionPreset::default(),
            storage: ArtifactStorageMode::default(),
        }
    }
}

impl ArtifactProfile {
    pub fn for_presets(
        chunk: ArtifactSizingPreset,
        compression: CompressionPreset,
        bundle: BundleSizingPreset,
        storage: ArtifactStorageMode,
    ) -> Self {
        Self {
            chunk,
            bundle,
            compression,
            storage,
        }
    }

    pub fn chunk_sizing_profile(&self) -> ChunkSizingProfile {
        ChunkSizingProfile::for_preset(self.chunk)
    }

    pub fn compression_profile(&self) -> CompressionProfile {
        CompressionProfile::for_preset(self.compression)
    }

    pub fn bundle_sizing_profile(&self) -> BundleSizingProfile {
        BundleSizingProfile::for_preset(self.bundle)
    }
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
    #[serde(rename = "content_packaging_profile")]
    pub artifact_profile: ArtifactProfile,
    #[serde(
        rename = "product_metadata",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub product_metadata: Option<RepoProductMetadata>,
    #[serde(rename = "snapshot_tag_schema")]
    pub snapshot_tag_schema: Option<SnapshotTagSchema>,
    #[serde(skip)]
    pub remote_repo_id: String,
    #[serde(skip)]
    pub product_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepoProductMetadata {
    #[serde(rename = "document_version")]
    pub document_version: String,
    pub product_id: String,
}

impl GutchRepoManifest {
    pub fn new(name: String) -> Self {
        Self {
            format: CONTENT_REPO_FORMAT.to_string(),
            format_version: FORMAT_VERSION.to_string(),
            name,
            created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            artifact_profile: ArtifactProfile::default(),
            product_metadata: None,
            snapshot_tag_schema: Some(SnapshotTagSchema::default()),
            remote_repo_id: String::new(),
            product_id: String::new(),
        }
    }

    pub fn set_artifact_profile(
        &mut self,
        chunk: ArtifactSizingPreset,
        compression: CompressionPreset,
        bundle: BundleSizingPreset,
        storage: ArtifactStorageMode,
    ) {
        self.artifact_profile = ArtifactProfile::for_presets(chunk, compression, bundle, storage);
    }

    pub fn compression_profile(&self) -> CompressionProfile {
        self.artifact_profile.compression_profile()
    }

    pub fn is_linked(&self) -> bool {
        !self.remote_repo_id.is_empty()
    }

    pub fn sync_product_metadata_from_binding(&mut self) {
        self.product_metadata = if self.product_id.trim().is_empty() {
            None
        } else {
            Some(RepoProductMetadata {
                document_version: FORMAT_VERSION.to_string(),
                product_id: self.product_id.clone(),
            })
        };
    }

    pub fn remote_repo_id_value(&self) -> Option<i64> {
        self.remote_repo_id.parse::<i64>().ok()
    }
}

#[cfg(test)]
#[path = "gutch_repo_tests.rs"]
mod tests;
