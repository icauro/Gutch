use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactProfile {
    pub chunk: ArtifactSizingPreset,
    pub compression: CompressionPreset,
    pub bundle: BundleSizingPreset,
    pub storage: ArtifactStorageMode,
}

impl Default for ArtifactProfile {
    fn default() -> Self {
        Self {
            chunk: ArtifactSizingPreset::default(),
            compression: CompressionPreset::default(),
            bundle: BundleSizingPreset::default(),
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
            compression,
            bundle,
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
pub struct GutchRepoManifest {
    pub format: String,
    pub format_version: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote_repo_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub product_id: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_schema: Option<TargetTagSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<launch::RepoLaunchConfiguration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_product_dependencies:
        Vec<external_dependency::ExternalProductDependencyRequirement>,
    pub artifact_profile: ArtifactProfile,
}

impl GutchRepoManifest {
    pub fn new(name: String) -> Self {
        Self {
            format: CONTENT_REPO_FORMAT.to_string(),
            format_version: FORMAT_VERSION.to_string(),
            name,
            remote_repo_id: String::new(),
            product_id: String::new(),
            created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            target_schema: None,
            launch: None,
            external_product_dependencies: Vec::new(),
            artifact_profile: ArtifactProfile::default(),
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

    pub fn remote_repo_id_value(&self) -> Option<i64> {
        self.remote_repo_id.parse::<i64>().ok()
    }
}

#[cfg(test)]
#[path = "gutch_repo_tests.rs"]
mod tests;
