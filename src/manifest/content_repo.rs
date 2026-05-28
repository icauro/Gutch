use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentRepoManifest {
    pub format: String,
    pub format_version: String,
    pub content_repo_id: String,
    pub created_at: String,
    pub targets_dir: String,
    pub bundles_dir: String,
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
}

impl ContentRepoManifest {
    pub fn new(content_repo_id: String) -> Self {
        Self {
            format: CONTENT_REPO_FORMAT.to_string(),
            format_version: FORMAT_VERSION.to_string(),
            content_repo_id,
            created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            targets_dir: "targets".to_string(),
            bundles_dir: "bundles".to_string(),
            target_schema: None,
            plugin: None,
            chunk_sizing_profile: ChunkSizingProfile::default(),
            compression_profile: CompressionProfile::default(),
            bundle_sizing_profile: BundleSizingProfile::default(),
        }
    }
}

#[cfg(test)]
#[path = "content_repo_tests.rs"]
mod tests;
