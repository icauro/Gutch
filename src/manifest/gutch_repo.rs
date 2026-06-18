use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GutchRepoManifest {
    pub format: String,
    pub format_version: String,
    #[serde(default, skip_serializing)]
    pub gutch_repo_id: String,
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
    pub fn new(gutch_repo_id: String) -> Self {
        Self {
            format: CONTENT_REPO_FORMAT.to_string(),
            format_version: FORMAT_VERSION.to_string(),
            gutch_repo_id,
            created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            target_schema: None,
            plugin: None,
            chunk_sizing_profile: ChunkSizingProfile::default(),
            compression_profile: CompressionProfile::default(),
            bundle_sizing_profile: BundleSizingProfile::default(),
            storage_mode: ArtifactStorageMode::default(),
        }
    }
}

#[cfg(test)]
#[path = "gutch_repo_tests.rs"]
mod tests;
