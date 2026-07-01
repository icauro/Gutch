use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{
    Result, SourceTargetInstallCandidate, build_source_target_install_candidate,
    inspect_gutch_repo, remove_target_from_repo,
};
use crate::manifest::TargetTagSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetInsertKind {
    InsertNew,
    UpdateTags,
    ReplaceSameTags,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInsertPlan {
    pub kind: TargetInsertKind,
    pub content_target_id: String,
    pub folder_name: String,
    pub path: String,
    pub version_id: String,
    pub os: String,
    pub arch: String,
    pub target_tags: TargetTagSet,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_tags: Option<TargetTagSet>,
    pub replaced_target_ids: Vec<String>,
    pub invalidates_existing_installs: bool,
}

impl TargetInsertPlan {
    pub fn from_candidate(
        candidate: SourceTargetInstallCandidate,
        kind: TargetInsertKind,
        from_tags: Option<TargetTagSet>,
        replaced_target_ids: Vec<String>,
    ) -> Self {
        let invalidates_existing_installs = kind == TargetInsertKind::ReplaceSameTags;
        Self {
            kind,
            content_target_id: candidate.content_target_id,
            folder_name: candidate.folder_name,
            path: candidate.path,
            version_id: candidate.version_id,
            os: candidate.os,
            arch: candidate.arch,
            target_tags: candidate.target_tags,
            from_tags,
            replaced_target_ids,
            invalidates_existing_installs,
        }
    }

    pub fn requires_confirmation(&self) -> bool {
        true
    }
}

pub fn plan_target_insert(
    repo_root: &Path,
    source_dir: &Path,
    explicit_tags: &TargetTagSet,
) -> Result<TargetInsertPlan> {
    let (_, repo_manifest) = crate::load_gutch_repo_manifest(repo_root)?;
    let rules = repo_manifest.target_schema.clone().unwrap_or_default();

    let candidate = build_source_target_install_candidate(source_dir, &rules, explicit_tags)?;
    let status = inspect_gutch_repo(repo_root)?;

    if let Some(existing) = status
        .targets
        .iter()
        .find(|target| target.target_id == candidate.content_target_id)
    {
        if existing.target_tags != candidate.target_tags {
            return Ok(TargetInsertPlan::from_candidate(
                candidate,
                TargetInsertKind::UpdateTags,
                Some(existing.target_tags.clone()),
                Vec::new(),
            ));
        }
        return Ok(TargetInsertPlan::from_candidate(
            candidate,
            TargetInsertKind::InsertNew,
            None,
            Vec::new(),
        ));
    }

    let replaced_target_ids: Vec<String> = status
        .targets
        .iter()
        .filter(|target| target.target_tags == candidate.target_tags)
        .map(|target| target.target_id.clone())
        .collect();

    if !replaced_target_ids.is_empty() {
        return Ok(TargetInsertPlan::from_candidate(
            candidate,
            TargetInsertKind::ReplaceSameTags,
            None,
            replaced_target_ids,
        ));
    }

    Ok(TargetInsertPlan::from_candidate(
        candidate,
        TargetInsertKind::InsertNew,
        None,
        Vec::new(),
    ))
}

pub fn delete_repo_targets(repo_root: &Path, target_ids: &[String]) -> Result<()> {
    for target_id in target_ids {
        remove_target_from_repo(repo_root, target_id)?;
    }
    Ok(())
}

pub fn delete_repo_targets_with_matching_tags(
    repo_root: &Path,
    target_tags: &TargetTagSet,
) -> Result<Vec<String>> {
    let status = inspect_gutch_repo(repo_root)?;
    let mut removed = Vec::new();
    for target in status.targets {
        if target.target_tags != *target_tags {
            continue;
        }
        remove_target_from_repo(repo_root, &target.target_id)?;
        removed.push(target.target_id);
    }
    Ok(removed)
}

pub fn describe_target_insert_plan(plan: &TargetInsertPlan) -> String {
    match plan.kind {
        TargetInsertKind::InsertNew => format!(
            "Insert new target {} ({})",
            plan.content_target_id, plan.folder_name
        ),
        TargetInsertKind::UpdateTags => format!(
            "Update tags for existing content {} ({}): folder rename or tag inference changed; content unchanged",
            plan.content_target_id, plan.folder_name
        ),
        TargetInsertKind::ReplaceSameTags => format!(
            "Replace target slot for {} ({}): same tags, new content id {}. Removes repo target(s) {}. Patron installs pinned to the old content id become invalid until updated.",
            plan.folder_name,
            plan.version_id,
            plan.content_target_id,
            plan.replaced_target_ids.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GutchRepoPaths;
    use crate::manifest::{self, target::TargetManifest};
    use crate::repo::{
        compute_target_id_from_files, ensure_gutch_repo_layout, write_target_manifest_artifacts,
    };

    #[test]
    fn plan_target_insert_detects_replace_same_tags() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let source_root = temp.path().join("sources");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let new_source = source_root.join("1.0.0-windows-x86_64-base");
        std::fs::create_dir_all(&new_source).unwrap();
        std::fs::write(new_source.join("game.exe"), b"old-content").unwrap();

        let (_, repo_manifest) = crate::load_gutch_repo_manifest(&repo_root).unwrap();
        let rules = repo_manifest.target_schema.clone().unwrap_or_default();
        let old_candidate = super::super::build_source_target_install_candidate(
            &new_source,
            &rules,
            &manifest::TargetTagSet::default(),
        )
        .unwrap();
        let old_files = vec![manifest::FileManifest {
            path: "game.exe".into(),
            size: 11,
            executable: false,
            full_file_hash: crate::hash::hash_hex(
                &crate::hash::hash_file(new_source.join("game.exe").as_path()).unwrap(),
            ),
            chunks: Vec::new(),
        }];
        let old_content_id = compute_target_id_from_files(&old_files);
        let mut old_manifest =
            TargetManifest::new(old_content_id.clone(), old_candidate.target_tags);
        old_manifest.files = old_files;
        write_target_manifest_artifacts(&repo_paths, &old_manifest).unwrap();

        std::fs::write(new_source.join("game.exe"), b"new-content").unwrap();

        let plan = plan_target_insert(&repo_root, &new_source, &manifest::TargetTagSet::default())
            .unwrap();
        assert_eq!(plan.kind, TargetInsertKind::ReplaceSameTags);
        assert_eq!(plan.replaced_target_ids, vec![old_content_id]);
        assert!(plan.invalidates_existing_installs);
    }
}
