use serde::{Deserialize, Serialize};

use super::*;

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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "auto_tagging_rules"
    )]
    pub target_schema: Option<TargetTagSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginInfo>,
    #[serde(default)]
    pub chunk_sizing_profile: ChunkSizingProfile,
    #[serde(default)]
    pub compression_profile: CompressionProfile,
    #[serde(default)]
    pub bundle_sizing_profile: BundleSizingProfile,
    #[serde(default)]
    pub storage_mode: ArtifactStorageMode,
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
            plugin: None,
            chunk_sizing_profile: ChunkSizingProfile::default(),
            compression_profile: CompressionProfile::default(),
            bundle_sizing_profile: BundleSizingProfile::default(),
            storage_mode: ArtifactStorageMode::default(),
        }
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
