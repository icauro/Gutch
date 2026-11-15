//! Discovery of configured source folders as capture candidates.

use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::Result;
use crate::manifest;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::snapshot_tag_summary::summarize_snapshot_tags;
use crate::snapshot::identity::source_modified_at_from_directory;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSnapshotOperationCandidate {
    pub path: String,
    pub folder_name: String,
    #[serde(default)]
    pub source_modified_at: String,
    pub version_tag: String,
    pub os: String,
    pub arch: String,
    #[serde(rename = "tags")]
    pub snapshot_tags: manifest::SnapshotTagSet,
    #[serde(default)]
    pub disposition: SourceSnapshotOperationDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSnapshotOperationDisposition {
    #[default]
    Unchanged,
    Capture,
    Invalid,
}

pub fn discover_source_snapshot_operations(
    repo_root: &Path,
    source_snapshots_root: &Path,
) -> Result<Vec<SourceSnapshotOperationCandidate>> {
    if !source_snapshots_root.exists() {
        return Err(Error::manifest(format!(
            "source snapshots path does not exist: {}",
            source_snapshots_root.display()
        )));
    }
    if !source_snapshots_root.is_dir() {
        return Err(Error::manifest(format!(
            "source snapshots path is not a directory: {}",
            source_snapshots_root.display()
        )));
    }

    load_gutch_repo_manifest(repo_root)?;
    let config = super::source_config::SourceConfig::load_or_default(source_snapshots_root)?;
    let mut candidates = Vec::new();

    for snapshot in config.discover_snapshot_sources(source_snapshots_root)? {
        let explicit_tags = snapshot.config_tags.clone().unwrap_or_default();
        match build_source_snapshot_ops_candidate(&snapshot.path, &explicit_tags) {
            Ok(candidate) => candidates.push(candidate),
            Err(err) => {
                let folder_name = snapshot
                    .path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_string();
                if !folder_name.is_empty() {
                    candidates.push(SourceSnapshotOperationCandidate {
                        path: snapshot.path.display().to_string(),
                        folder_name,
                        source_modified_at: String::new(),
                        version_tag: String::new(),
                        os: String::new(),
                        arch: String::new(),
                        snapshot_tags: manifest::SnapshotTagSet::default(),
                        disposition: SourceSnapshotOperationDisposition::Invalid,
                        error: Some(err.to_string()),
                    });
                }
            }
        }
    }

    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(candidates)
}

pub(crate) fn build_source_snapshot_ops_candidate(
    path: &Path,
    explicit_tags: &manifest::SnapshotTagSet,
) -> Result<SourceSnapshotOperationCandidate> {
    let source_modified_at = source_modified_at_from_directory(path)?;
    build_source_snapshot_ops_candidate_with_modified_at(path, explicit_tags, source_modified_at)
}

pub(crate) fn build_source_snapshot_ops_candidate_with_modified_at(
    path: &Path,
    explicit_tags: &manifest::SnapshotTagSet,
    source_modified_at: String,
) -> Result<SourceSnapshotOperationCandidate> {
    let normalized_tags = explicit_tags.normalized()?;

    let folder_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            Error::manifest(format!("invalid source snapshot path: {}", path.display()))
        })?
        .to_string();

    let summary = summarize_snapshot_tags(&normalized_tags);

    Ok(SourceSnapshotOperationCandidate {
        path: path.display().to_string(),
        folder_name,
        source_modified_at,
        version_tag: summary.version_tag,
        os: summary.os,
        arch: summary.arch,
        snapshot_tags: normalized_tags,
        disposition: SourceSnapshotOperationDisposition::Unchanged,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_source_snapshot_operations_returns_matching_subfolders_only() {
        let root = std::env::temp_dir().join(format!(
            "gutch_source_discover_test_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let repo_root = root.join("repo");
        let source_root = root.join("source-snapshots");
        let repo_paths = crate::repo::paths::RepoPaths::from_repo_root(&repo_root);
        let _ = std::fs::remove_dir_all(&root);

        crate::repo::paths::ensure_gutch_repo_layout(&repo_paths, "mods").unwrap();

        let repo_manifest = crate::manifest::repo_manifest::RepoManifest::new("mods".to_string());
        std::fs::write(
            &repo_paths.repo_metadata_path,
            serde_json::to_string_pretty(&repo_manifest).unwrap(),
        )
        .unwrap();

        std::fs::create_dir_all(source_root.join("ae2-v15.4.10-loader-fabric-mc-1.21")).unwrap();
        std::fs::create_dir_all(source_root.join("not-a-snapshot-folder")).unwrap();

        let candidates = discover_source_snapshot_operations(&repo_root, &source_root).unwrap();

        assert_eq!(candidates.len(), 2);
        // Folder names are not interpreted as tags.
        assert_eq!(
            candidates[0].folder_name,
            "ae2-v15.4.10-loader-fabric-mc-1.21"
        );
        assert_eq!(candidates[0].version_tag, "");
        assert_eq!(
            candidates[0].disposition,
            SourceSnapshotOperationDisposition::Unchanged
        );
        // Every discovered directory is a valid tagless source.
        assert_eq!(candidates[1].folder_name, "not-a-snapshot-folder");
        assert_eq!(
            candidates[1].disposition,
            SourceSnapshotOperationDisposition::Unchanged
        );
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.snapshot_tags.is_empty())
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
