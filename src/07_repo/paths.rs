//! Repository layout, artifact paths, the repository manifest and local repository config.

use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::filesystem;
use crate::filesystem::write_json;
use crate::manifest::REPO_METADATA_FILE;
use crate::manifest::repo_manifest::RepoManifest;
use std::fs;

/// Coordination lock file at the repository root.
pub(crate) const LOCK_FILE: &str = ".gutch-lock";
/// Transaction journal directory at the repository root.
pub(crate) const TRANSACTIONS_DIR: &str = ".gutch-transactions";

#[derive(Debug, Clone)]
pub struct RepoPaths {
    pub(crate) repo_root: PathBuf,
    pub(crate) repo_metadata_path: PathBuf,
    pub(crate) local_repo_config_path: PathBuf,
    pub(crate) chunk_map_path: PathBuf,
    pub(crate) snapshots_root: PathBuf,
    pub(crate) bundles_root: PathBuf,
    pub(crate) chunks_root: PathBuf,
}

impl RepoPaths {
    pub fn from_store_root(store_root: &Path, gutch_repo_id: &str) -> Result<Self> {
        validate_path_component(gutch_repo_id, "Gutch repo ID")?;
        let repo_root = store_root.join("v1").join("repos").join(gutch_repo_id);
        Ok(Self::from_repo_root(&repo_root))
    }

    /// Every top-level path the repository owns; other tools must not write inside them.
    pub(crate) fn owned_entries(&self) -> [PathBuf; 8] {
        [
            self.repo_metadata_path.clone(),
            self.local_repo_config_path.clone(),
            self.chunk_map_path.clone(),
            self.snapshots_root.clone(),
            self.bundles_root.clone(),
            self.chunks_root.clone(),
            self.repo_root.join(LOCK_FILE),
            self.repo_root.join(TRANSACTIONS_DIR),
        ]
    }

    pub fn from_repo_root(repo_root: &Path) -> Self {
        Self {
            repo_root: repo_root.to_path_buf(),
            repo_metadata_path: repo_root.join(REPO_METADATA_FILE),
            local_repo_config_path: repo_root.join("local_repo_config.json"),
            chunk_map_path: repo_root.join("chunk_map.gtcbin"),
            snapshots_root: repo_root.join("snapshots"),
            bundles_root: repo_root.join("bundles"),
            chunks_root: repo_root.join("chunks"),
        }
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }
}

pub(crate) fn validate_path_component(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('\0')
        || value.contains('/')
        || value.contains('\\')
        || value.contains(':')
        || Path::new(value).components().count() != 1
    {
        return Err(Error::repository(format!(
            "invalid {label} path component '{value}'"
        )));
    }
    Ok(())
}

pub(crate) fn ensure_gutch_repo_layout(paths: &RepoPaths, name: &str) -> Result<RepoManifest> {
    filesystem::create_dir_all(&paths.snapshots_root)?;
    filesystem::create_dir_all(&paths.bundles_root)?;
    filesystem::create_dir_all(&paths.chunks_root)?;

    let repo_manifest = if paths.repo_metadata_path.exists() {
        let mut manifest: RepoManifest = filesystem::read_json(&paths.repo_metadata_path)?;
        apply_local_repo_config(paths, &mut manifest)?;
        manifest
    } else {
        let repo_manifest = RepoManifest::new(name.to_string());
        write_json(&paths.repo_metadata_path, &repo_manifest)?;
        repo_manifest
    };

    Ok(repo_manifest)
}

pub fn load_gutch_repo_manifest(repo_root: &Path) -> Result<(RepoPaths, RepoManifest)> {
    let paths = RepoPaths::from_repo_root(repo_root);
    if !paths.repo_metadata_path.exists() {
        return Err(Error::repository(format!(
            "gutch repo manifest not found at {}",
            paths.repo_metadata_path.display()
        )));
    }
    let mut repo_manifest: RepoManifest = filesystem::read_json(&paths.repo_metadata_path)?;
    if let Some(constraints) = &repo_manifest.constraints {
        constraints.validate_definition()?;
    }
    apply_local_repo_config(&paths, &mut repo_manifest)?;
    Ok((paths, repo_manifest))
}

pub(crate) fn write_gutch_repo_manifest(
    paths: &RepoPaths,
    repo_manifest: &RepoManifest,
) -> Result<()> {
    write_json(&paths.repo_metadata_path, repo_manifest)?;
    match &repo_manifest.remote_repo_id {
        None => {
            if paths.local_repo_config_path.exists() {
                fs::remove_file(&paths.local_repo_config_path)?;
            }
        }
        Some(remote_repo_id) => write_json(
            &paths.local_repo_config_path,
            &LocalRepoConfig {
                remote_repo_id: remote_repo_id.clone(),
            },
        )?,
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalRepoConfig {
    remote_repo_id: String,
}

fn apply_local_repo_config(paths: &RepoPaths, manifest: &mut RepoManifest) -> Result<()> {
    if !paths.local_repo_config_path.exists() {
        return Ok(());
    }
    let config: LocalRepoConfig = filesystem::read_json(&paths.local_repo_config_path)?;
    let remote_repo_id = config.remote_repo_id.trim();
    if remote_repo_id.is_empty() {
        return Err(Error::repository(format!(
            "{} has an empty remote_repo_id",
            paths.local_repo_config_path.display()
        )));
    }
    manifest.remote_repo_id = Some(remote_repo_id.to_owned());
    Ok(())
}

pub(crate) fn bundle_path_in_root(bundles_root: &Path, bundle_id: &str) -> Result<PathBuf> {
    validate_path_component(bundle_id, "bundle ID")?;
    let prefix = bundle_id.get(0..2).unwrap_or("xx");
    Ok(bundles_root.join(prefix).join(bundle_id))
}

pub(crate) fn bundle_path(paths: &RepoPaths, bundle_id: &str) -> Result<PathBuf> {
    bundle_path_in_root(&paths.bundles_root, bundle_id)
}

/// Loose chunks are sharded by the first two hex digits of their hash.
pub(crate) fn chunk_path_in_root(chunks_root: &Path, chunk_hash: ChunkHash) -> PathBuf {
    let hex = chunk_hash.to_hex();
    chunks_root.join(&hex[..2]).join(hex)
}

pub(crate) fn chunk_path(paths: &RepoPaths, chunk_hash: ChunkHash) -> PathBuf {
    chunk_path_in_root(&paths.chunks_root, chunk_hash)
}

pub(crate) fn bundle_id_from_bundle_path(path: &Path) -> Option<String> {
    let bundle_id = path.file_name().and_then(|value| value.to_str())?;
    ChunkHash::parse_hex(bundle_id).ok()?;
    Some(bundle_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_components() {
        let store_root = Path::new("store");
        assert!(RepoPaths::from_store_root(store_root, "../escape").is_err());
        assert!(bundle_path_in_root(store_root, "..\\escape").is_err());
        assert!(validate_path_component("C:\\escape", "component").is_err());
    }

    #[test]
    fn accepts_single_component_identifiers() {
        let paths = RepoPaths::from_store_root(Path::new("store"), "repo-1").unwrap();
        assert!(paths.repo_root().ends_with(Path::new("v1/repos/repo-1")));
        assert!(bundle_path(&paths, "bundle-1").is_ok());
    }

    #[test]
    fn shards_bundle_paths_by_the_first_two_identifier_characters() {
        let bundles_root = Path::new("repo/bundles");
        let bundle_id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        assert_eq!(
            bundle_path_in_root(bundles_root, bundle_id).unwrap(),
            bundles_root.join("ab").join(bundle_id)
        );
    }
}
