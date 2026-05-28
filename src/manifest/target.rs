use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetManifest {
    pub manifest_version: String,
    pub content_repo_id: String,
    pub target_id: String,
    pub created_at: String,
    pub total_logical_size: u64,
    pub total_compressed_size_estimate: u64,
    #[serde(default)]
    pub target_tags: TargetTagSet,
    pub files: Vec<FileManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierOverlayManifest {
    pub format_version: String,
    pub version_id: String,
    pub base_tier_id: String,
    pub target_tier_id: String,
    pub add_files: Vec<FileManifest>,
    pub replace_files: Vec<FileManifest>,
}

impl TargetManifest {
    pub fn new(content_repo_id: String, target_id: String, target_tags: TargetTagSet) -> Self {
        TargetManifest {
            manifest_version: FORMAT_VERSION.to_string(),
            content_repo_id,
            target_id,
            created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            total_logical_size: 0,
            total_compressed_size_estimate: 0,
            target_tags,
            files: Vec::new(),
        }
    }
}

#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
