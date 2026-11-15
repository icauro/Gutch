//! Repository initialization: risk assessment of the target path and confirmation policy.

use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

use super::read_repo_status;
use crate::Error;
use crate::Result;
use crate::filesystem;
use crate::manifest::constraints::RepositoryConstraints;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::ensure_gutch_repo_layout;
use crate::repo::paths::write_gutch_repo_manifest;
use std::fs;

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
    pub snapshot_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct RepoInitOptions {
    pub force: bool,
    pub constraints: RepositoryConstraints,
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
            snapshot_count: None,
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
            snapshot_count: None,
            bundle_count: None,
            linked: None,
        });
    }

    let entry_count = fs::read_dir(repo_root)?
        .filter_map(|entry| entry.ok())
        .count();
    let paths = RepoPaths::from_repo_root(repo_root);

    if paths.repo_metadata_path.exists() {
        // An unreadable manifest is what `init --force` replaces, so classify it
        // here instead of failing the assessment. I/O failures still propagate.
        let manifest_error = match crate::repo::paths::load_gutch_repo_manifest(repo_root) {
            Ok(_) => None,
            Err(error @ (Error::Io(_) | Error::IoContext { .. } | Error::Cancelled)) => {
                return Err(error);
            }
            Err(error) => Some(error.to_string()),
        };
        let status = match manifest_error {
            None => read_repo_status(repo_root)?,
            Some(error) => {
                return Ok(RepoInitAssessment {
                    path,
                    path_kind: RepoInitPathKind::InvalidManifest,
                    requires_confirmation: true,
                    risks: vec![invalid_manifest_risk(Some(&error))],
                    entry_count,
                    repo_name: None,
                    snapshot_count: None,
                    bundle_count: None,
                    linked: None,
                });
            }
        };
        if status.is_repo {
            return Ok(RepoInitAssessment {
                path,
                path_kind: RepoInitPathKind::ExistingRepo,
                requires_confirmation: true,
                risks: vec![existing_repo_risk()],
                entry_count,
                repo_name: Some(status.name),
                snapshot_count: Some(status.snapshots.len()),
                bundle_count: Some(status.bundle_count),
                linked: Some(!status.remote_repo_id.is_empty()),
            });
        }

        return Ok(RepoInitAssessment {
            path,
            path_kind: RepoInitPathKind::InvalidManifest,
            requires_confirmation: true,
            risks: vec![invalid_manifest_risk(status.error.as_deref())],
            entry_count,
            repo_name: None,
            snapshot_count: None,
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
            snapshot_count: None,
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
        snapshot_count: None,
        bundle_count: None,
        linked: None,
    })
}

pub fn initialize_repo(repo_root: &Path, options: RepoInitOptions) -> Result<()> {
    options.constraints.validate_definition()?;
    let assessment = assess_repo_init_path(repo_root)?;

    if assessment.path_kind == RepoInitPathKind::NotADirectory {
        return Err(Error::repository(
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

    fs::create_dir_all(repo_root)?;
    super::coordination::write(repo_root, || {
        if !options.force && repo_root.join("repo_metadata.json").exists() {
            return Err(Error::repository(
                "repository was initialized by another writer",
            ));
        }
        if options.force && assessment.path_kind != RepoInitPathKind::Missing {
            clear_gutch_repo_artifacts(repo_root)?;
        }

        let repo_name = repo_root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("base");
        let repo_paths = RepoPaths::from_repo_root(repo_root);
        let mut repo_manifest = ensure_gutch_repo_layout(&repo_paths, repo_name)?;
        repo_manifest.constraints =
            (!options.constraints.is_empty()).then_some(options.constraints);
        write_gutch_repo_manifest(&repo_paths, &repo_manifest)?;

        Ok(())
    })
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

pub(crate) fn repo_init_needs_confirmation_error(assessment: &RepoInitAssessment) -> Error {
    let payload = serde_json::to_string(assessment).unwrap_or_else(|_| "{}".to_string());
    Error::repository(format!("{REPO_INIT_NEEDS_CONFIRMATION}:{payload}"))
}

pub(super) fn clear_gutch_repo_artifacts(repo_root: &Path) -> Result<()> {
    let paths = RepoPaths::from_repo_root(repo_root);
    for (_, artifact) in super::inventory::list_repo_artifact_paths(&paths)? {
        if !super::coordination::defer_artifact_deletion(&artifact)? {
            fs::remove_file(artifact)?;
        }
    }
    if paths.snapshots_root.exists() {
        filesystem::remove_dir_all(&paths.snapshots_root)?;
    }
    if paths.repo_metadata_path.exists() {
        filesystem::remove_file(&paths.repo_metadata_path)?;
    }
    if paths.local_repo_config_path.exists() {
        filesystem::remove_file(&paths.local_repo_config_path)?;
    }
    if paths.chunk_map_path.exists() {
        filesystem::remove_file(&paths.chunk_map_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;
    use crate::manifest::snapshot::SnapshotManifest;
    use crate::repo::paths::ensure_gutch_repo_layout;
    use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;

    fn exclusive_tags(version: &str, os: &str, arch: &str, tier: &str) -> manifest::SnapshotTagSet {
        let mut tags = manifest::SnapshotTagSet::default();
        tags.insert("version", version).unwrap();
        tags.insert("os", os).unwrap();
        tags.insert("arch", arch).unwrap();
        tags.insert("tier", tier).unwrap();
        tags
    }

    fn test_hash(label: &str) -> String {
        crate::artifact::hash::ChunkHash::of(label.as_bytes()).to_hex()
    }

    #[test]
    fn assess_repo_init_path_reports_existing_repo() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let repo_paths = RepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let assessment = assess_repo_init_path(&repo_root).unwrap();
        assert_eq!(assessment.path_kind, RepoInitPathKind::ExistingRepo);
        assert!(assessment.requires_confirmation);
    }

    #[test]
    fn initialize_repo_init_without_force_rejects_existing_repo() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let repo_paths = RepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

        let err = initialize_repo(
            &repo_root,
            RepoInitOptions {
                force: false,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains(REPO_INIT_NEEDS_CONFIRMATION));
    }

    #[test]
    fn initialize_repo_init_with_force_replaces_existing_repo_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let repo_paths = RepoPaths::from_repo_root(&repo_root);
        ensure_gutch_repo_layout(&repo_paths, "base").unwrap();
        let snapshot_id = test_hash("snapshot-one");
        let mut snapshot_manifest = SnapshotManifest::new(
            snapshot_id.clone(),
            exclusive_tags("1.0.0", "windows", "x86_64", "base"),
        );
        snapshot_manifest.push_file(manifest::FileManifest {
            path: "game.exe".to_string(),
            size: 4,
            executable: true,
            read_only: false,
            full_file_hash: test_hash("game.exe"),
            chunks: vec![crate::manifest::ChunkRef {
                chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&test_hash("game.exe"))
                    .unwrap(),
                raw_size: 4,
            }],
        });
        snapshot_manifest.snapshot_id =
            crate::snapshot::identity::compute_snapshot_id_from_entries(&snapshot_manifest.entries);
        write_snapshot_manifest_artifacts(
            &repo_paths,
            &snapshot_manifest,
            &crate::progress::OperationControl::default(),
        )
        .unwrap();
        crate::test_util::index_manifest(&repo_paths, &snapshot_manifest);
        let bundle_path = crate::repo::paths::bundle_path(&repo_paths, &"a".repeat(64)).unwrap();
        std::fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
        std::fs::write(bundle_path, b"bundle").unwrap();

        initialize_repo(
            &repo_root,
            RepoInitOptions {
                force: true,
                ..Default::default()
            },
        )
        .unwrap();

        let status = read_repo_status(&repo_root).unwrap();
        assert!(status.is_repo);
        assert_eq!(status.snapshots.len(), 0);
        assert_eq!(status.bundle_count, 0);
    }

    #[test]
    fn initialize_repo_init_with_force_keeps_unrelated_files_in_non_empty_path() {
        let temp = tempfile::tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::write(repo_root.join("notes.txt"), b"keep me").unwrap();

        initialize_repo(
            &repo_root,
            RepoInitOptions {
                force: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(repo_root.join("notes.txt").exists());
        assert!(repo_root.join("snapshots").is_dir());
    }
}
