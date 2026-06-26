use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{
    Result, SourceTargetInstallCandidate, discover_source_target_installs, inspect_gutch_repo,
};
use crate::manifest::TargetTagSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSourcesTagUpdate {
    pub content_target_id: String,
    pub folder_name: String,
    pub path: String,
    pub from_tags: TargetTagSet,
    pub to_tags: TargetTagSet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSourcesSyncPlan {
    pub sources_ready: bool,
    pub candidate_count: usize,
    pub insertable_count: usize,
    pub tag_update_count: usize,
    pub removable_count: usize,
    #[serde(default)]
    pub to_insert: Vec<SourceTargetInstallCandidate>,
    #[serde(default)]
    pub to_update_tags: Vec<TargetSourcesTagUpdate>,
    #[serde(default)]
    pub to_remove: Vec<String>,
}

impl TargetSourcesSyncPlan {
    fn empty() -> Self {
        Self {
            sources_ready: false,
            candidate_count: 0,
            insertable_count: 0,
            tag_update_count: 0,
            removable_count: 0,
            to_insert: Vec::new(),
            to_update_tags: Vec::new(),
            to_remove: Vec::new(),
        }
    }

    pub fn has_changes(&self) -> bool {
        self.insertable_count > 0 || self.tag_update_count > 0 || self.removable_count > 0
    }

    pub fn refresh_count(&self) -> usize {
        self.candidate_count
            .saturating_sub(self.insertable_count)
            .saturating_sub(self.tag_update_count)
    }
}

pub fn plan_target_sources_sync(
    repo_root: &Path,
    sources_path: Option<&Path>,
) -> Result<TargetSourcesSyncPlan> {
    let Some(sources_path) = normalized_sources_path(sources_path) else {
        return Ok(TargetSourcesSyncPlan::empty());
    };

    let mut plan = TargetSourcesSyncPlan {
        sources_ready: true,
        ..TargetSourcesSyncPlan::empty()
    };

    let candidates = match discover_source_target_installs(repo_root, &sources_path) {
        Ok(candidates) => candidates,
        Err(_) => return Ok(plan),
    };

    let status = inspect_gutch_repo(repo_root)?;
    let candidate_content_ids: HashSet<String> = candidates
        .iter()
        .map(|candidate| candidate.content_target_id.clone())
        .collect();

    plan.candidate_count = candidates.len();
    plan.to_remove = status
        .targets
        .iter()
        .filter(|target| !candidate_content_ids.contains(&target.target_id))
        .map(|target| target.target_id.clone())
        .collect();
    plan.removable_count = plan.to_remove.len();

    for candidate in candidates {
        let Some(repo_target) = status
            .targets
            .iter()
            .find(|target| target.target_id == candidate.content_target_id)
        else {
            plan.to_insert.push(candidate);
            continue;
        };

        if repo_target.target_tags != candidate.target_tags {
            plan.to_update_tags.push(TargetSourcesTagUpdate {
                content_target_id: candidate.content_target_id,
                folder_name: candidate.folder_name,
                path: candidate.path,
                from_tags: repo_target.target_tags.clone(),
                to_tags: candidate.target_tags,
            });
        }
    }

    plan.insertable_count = plan.to_insert.len();
    plan.tag_update_count = plan.to_update_tags.len();

    Ok(plan)
}

fn normalized_sources_path(sources_path: Option<&Path>) -> Option<std::path::PathBuf> {
    let sources_path = sources_path?;
    let trimmed = sources_path.to_string_lossy().trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    Some(std::path::PathBuf::from(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GutchRepoPaths;
    use crate::manifest::{self, target::TargetManifest};
    use crate::repo::{
        compute_target_id_from_files, ensure_gutch_repo_layout, write_target_manifest_artifacts,
    };

    fn exclusive_tags(version: &str, os: &str, arch: &str, tier: &str) -> manifest::TargetTagSet {
        let mut tags = manifest::TargetTagSet::default();
        tags.upsert_exclusive("version", version);
        tags.upsert_exclusive("os", os);
        tags.upsert_exclusive("arch", arch);
        tags.upsert_exclusive("tier", tier);
        tags
    }

    #[test]
    fn plan_target_sources_sync_returns_empty_plan_without_sources_path() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let plan = plan_target_sources_sync(&repo_root, None).unwrap();
        assert!(!plan.sources_ready);
        assert_eq!(plan.insertable_count, 0);
        assert_eq!(plan.tag_update_count, 0);
        assert_eq!(plan.removable_count, 0);
    }

    #[test]
    fn plan_target_sources_sync_detects_tag_update_for_same_content() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let source_root = temp.path().join("sources");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let source_dir = source_root.join("2.0.0-windows-x86_64-base");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("game.exe"), b"same-content").unwrap();

        let files = vec![manifest::FileManifest {
            path: "game.exe".into(),
            size: 12,
            executable: false,
            full_file_hash: crate::hash::hash_hex(&crate::hash::hash_file(
                &source_dir.join("game.exe"),
            )
            .unwrap()),
            chunks: Vec::new(),
        }];
        let content_target_id = compute_target_id_from_files(&files);
        let old_tags = exclusive_tags("1.0.0", "windows", "x86_64", "base");
        let mut manifest = TargetManifest::new(content_target_id.clone(), old_tags);
        manifest.files = files;
        manifest.total_logical_size = 12;
        write_target_manifest_artifacts(&repo_paths, &manifest).unwrap();

        let plan = plan_target_sources_sync(&repo_root, Some(&source_root)).unwrap();
        assert!(plan.sources_ready);
        assert_eq!(plan.candidate_count, 1);
        assert_eq!(plan.insertable_count, 0);
        assert_eq!(plan.tag_update_count, 1);
        assert_eq!(plan.removable_count, 0);
        assert_eq!(plan.to_update_tags[0].content_target_id, content_target_id);
        assert_eq!(
            plan.to_update_tags[0]
                .from_tags
                .get_exclusive("version")
                .map(str::to_string),
            Some("1.0.0".to_string())
        );
        assert_eq!(
            plan.to_update_tags[0]
                .to_tags
                .get_exclusive("version")
                .map(str::to_string),
            Some("2.0.0".to_string())
        );
        assert!(plan.has_changes());
    }
}
