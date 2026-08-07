use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{
    GutchRepoPaths, check_gutch_repo, ensure_gutch_repo_layout, write_gutch_repo_manifest,
};
use crate::manifest;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RepoInitPathKind {
    Missing,
    Empty,
    NonEmpty,
    ExistingRepo,
    InvalidManifest,
    NotADirectory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInitRisk {
    pub code: String,
    pub message: String,
}

pub const REPO_INIT_NEEDS_CONFIRMATION: &str = "repo_init_needs_confirmation";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInitAssessment {
    pub path: String,
    pub path_kind: RepoInitPathKind,
    pub requires_confirmation: bool,
    pub risks: Vec<RepoInitRisk>,
    pub entry_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RepoInitOptions {
    pub force: bool,
}

pub fn assess_repo_init_path(repo_root: &Path) -> Result<RepoInitAssessment> {
    let path = repo_root.display().to_string();

    if repo_root.exists() && !repo_root.is_dir() {
        return Ok(RepoInitAssessment {
            path,
            path_kind: RepoInitPathKind::NotADirectory,
            requires_confirmation: false,
            risks: vec![RepoInitRisk {
                code: "not_a_directory".to_string(),
                message: "Repo path is not a directory.".to_string(),
            }],
            entry_count: 0,
            repo_name: None,
            target_count: None,
            bundle_count: None,
            linked: None,
        });
    }

    if !repo_root.exists() {
        return Ok(RepoInitAssessment {
            path,
            path_kind: RepoInitPathKind::Missing,
            requires_confirmation: false,
            risks: Vec::new(),
            entry_count: 0,
            repo_name: None,
            target_count: None,
            bundle_count: None,
            linked: None,
        });
    }

    let entry_count = std::fs::read_dir(repo_root)?
        .filter_map(|entry| entry.ok())
        .count();
    let paths = GutchRepoPaths::from_repo_root(repo_root);

    if paths.repo_metadata_path.exists() {
        let status = check_gutch_repo(repo_root)?;
        if status.is_repo {
            return Ok(RepoInitAssessment {
                path,
                path_kind: RepoInitPathKind::ExistingRepo,
                requires_confirmation: true,
                risks: vec![existing_repo_risk()],
                entry_count,
                repo_name: Some(status.name),
                target_count: Some(status.targets.len()),
                bundle_count: Some(status.bundle_count),
                linked: Some(!status.remote_repo_id.is_empty() || !status.product_id.is_empty()),
            });
        }

        return Ok(RepoInitAssessment {
            path,
            path_kind: RepoInitPathKind::InvalidManifest,
            requires_confirmation: true,
            risks: vec![invalid_manifest_risk(status.error.as_deref())],
            entry_count,
            repo_name: None,
            target_count: None,
            bundle_count: None,
            linked: None,
        });
    }

    if entry_count == 0 {
        return Ok(RepoInitAssessment {
            path,
            path_kind: RepoInitPathKind::Empty,
            requires_confirmation: false,
            risks: Vec::new(),
            entry_count: 0,
            repo_name: None,
            target_count: None,
            bundle_count: None,
            linked: None,
        });
    }

    Ok(RepoInitAssessment {
        path,
        path_kind: RepoInitPathKind::NonEmpty,
        requires_confirmation: true,
        risks: vec![non_empty_risk(entry_count)],
        entry_count,
        repo_name: None,
        target_count: None,
        bundle_count: None,
        linked: None,
    })
}

pub fn initialize_tracked_gutch_repo_with_options(
    repo_root: &Path,
    target_schema: Option<manifest::TargetTagSchema>,
    target_sources_path: Option<String>,
    options: RepoInitOptions,
) -> Result<()> {
    let assessment = assess_repo_init_path(repo_root)?;

    if assessment.path_kind == RepoInitPathKind::NotADirectory {
        return Err(Error::manifest(
            assessment
                .risks
                .first()
                .map(|risk| risk.message.clone())
                .unwrap_or_else(|| "Repo path is not a directory.".to_string()),
        ));
    }

    if assessment.requires_confirmation && !options.force {
        return Err(repo_init_needs_confirmation_error(&assessment));
    }

    if options.force && assessment.path_kind != RepoInitPathKind::Missing {
        clear_gutch_repo_artifacts(repo_root)?;
    }

    let repo_name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("base");
    let repo_paths = GutchRepoPaths::from_repo_root(repo_root);
    let mut repo_manifest = ensure_gutch_repo_layout(&repo_paths, repo_name)?;
    repo_manifest.target_schema = target_schema;
    write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;

    let mut config = crate::config::load_config()?;
    config.upsert_repo(crate::config::TrackedRepo {
        path: repo_paths.repo_root.display().to_string(),
        target_sources_path,
    });
    crate::config::save_config(&config)?;
    Ok(())
}

fn existing_repo_risk() -> RepoInitRisk {
    RepoInitRisk {
        code: "existing_repo".to_string(),
        message: "This path already contains a Gutch repo. Continuing will delete all Gutch repo files and create a new repo.".to_string(),
    }
}

fn invalid_manifest_risk(error: Option<&str>) -> RepoInitRisk {
    let detail = error
        .filter(|value| !value.is_empty())
        .map(|value| format!(" ({value})"))
        .unwrap_or_default();
    RepoInitRisk {
        code: "invalid_manifest".to_string(),
        message: format!(
            "This path contains repo_metadata.json but it is not a valid Gutch repo{detail}. Continuing will delete all Gutch repo files and create a new repo."
        ),
    }
}

fn non_empty_risk(entry_count: usize) -> RepoInitRisk {
    RepoInitRisk {
        code: "non_empty".to_string(),
        message: format!(
            "This path is not empty ({entry_count} item{}). Continuing will remove any existing Gutch repo files before creating a new repo.",
            if entry_count == 1 { "" } else { "s" }
        ),
    }
}

pub fn repo_init_needs_confirmation_error(assessment: &RepoInitAssessment) -> Error {
    let payload = serde_json::to_string(assessment).unwrap_or_else(|_| "{}".to_string());
    Error::manifest(format!("{REPO_INIT_NEEDS_CONFIRMATION}:{payload}"))
}

pub fn parse_repo_init_needs_confirmation_error(message: &str) -> Option<RepoInitAssessment> {
    let payload = message.strip_prefix(&format!("{REPO_INIT_NEEDS_CONFIRMATION}:"))?;
    serde_json::from_str(payload).ok()
}

pub(super) fn clear_gutch_repo_artifacts(repo_root: &Path) -> Result<()> {
    let paths = GutchRepoPaths::from_repo_root(repo_root);
    for dir in [&paths.targets_root, &paths.bundles_root, &paths.chunks_root] {
        if dir.exists() {
            crate::helpers::remove_dir_all(dir)?;
        }
    }
    if paths.repo_metadata_path.exists() {
        crate::helpers::remove_file(&paths.repo_metadata_path)?;
    }
    if paths.chunk_map_path.exists() {
        crate::helpers::remove_file(&paths.chunk_map_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_hex;
    use crate::manifest::target::TargetManifest;
    use crate::{ensure_gutch_repo_layout, write_target_manifest_artifacts};

    fn exclusive_tags(version: &str, os: &str, arch: &str, tier: &str) -> manifest::TargetTagSet {
        let mut tags = manifest::TargetTagSet::default();
        tags.upsert_exclusive("version", version);
        tags.upsert_exclusive("os", os);
        tags.upsert_exclusive("arch", arch);
        tags.upsert_exclusive("tier", tier);
        tags
    }

    fn test_hash(label: &str) -> String {
        hash_hex(&crate::hash::hash_chunk(label.as_bytes()))
    }

    #[test]
    fn assess_repo_init_path_reports_existing_repo() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let assessment = assess_repo_init_path(&repo_root).unwrap();
        assert_eq!(assessment.path_kind, RepoInitPathKind::ExistingRepo);
        assert!(assessment.requires_confirmation);
    }

    #[test]
    fn initialize_repo_init_without_force_rejects_existing_repo() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let err = initialize_tracked_gutch_repo_with_options(
            &repo_root,
            None,
            None,
            RepoInitOptions { force: false },
        )
        .unwrap_err();
        assert!(err.to_string().contains(REPO_INIT_NEEDS_CONFIRMATION));
    }

    #[test]
    fn initialize_repo_init_with_force_replaces_existing_repo_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let repo_paths = GutchRepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();
        let mut target_manifest = TargetManifest::new(
            "1".to_string(),
            exclusive_tags("1.0.0", "windows", "x86_64", "base"),
        );
        target_manifest.files.push(manifest::FileManifest {
            path: "game.exe".to_string(),
            size: 4,
            executable: true,
            full_file_hash: test_hash("game.exe"),
            chunks: vec![],
        });
        write_target_manifest_artifacts(&repo_paths, &target_manifest).unwrap();
        std::fs::write(repo_paths.bundles_root.join("bundle-a"), b"bundle").unwrap();

        initialize_tracked_gutch_repo_with_options(
            &repo_root,
            None,
            None,
            RepoInitOptions { force: true },
        )
        .unwrap();

        let status = check_gutch_repo(&repo_root).unwrap();
        assert!(status.is_repo);
        assert_eq!(status.targets.len(), 0);
        assert_eq!(status.bundle_count, 0);
    }

    #[test]
    fn initialize_repo_init_with_force_keeps_unrelated_files_in_non_empty_path() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::write(repo_root.join("notes.txt"), b"keep me").unwrap();

        initialize_tracked_gutch_repo_with_options(
            &repo_root,
            None,
            None,
            RepoInitOptions { force: true },
        )
        .unwrap();

        assert!(repo_root.join("notes.txt").exists());
        assert!(repo_root.join("targets").is_dir());
    }
}
