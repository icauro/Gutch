//! Repository layout, artifact paths, the repository manifest and local repository config.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;

use crate::Error;
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::artifact::seal::SealKeys;
use crate::filesystem;
use crate::filesystem::write_json;
use crate::manifest::REPO_METADATA_FILE;
use crate::manifest::repo_manifest::RepoManifest;
use crate::manifest::{
    DocumentType, DocumentVersion, RepoFormat, RepositoryAccess, VersionedDocument,
    check_document_markers, parse_document,
};
use std::fs;

/// Coordination lock file at the repository root.
pub(crate) const LOCK_FILE: &str = ".gutch-lock";
/// Transaction journal directory at the repository root.
pub(crate) const TRANSACTIONS_DIR: &str = ".gutch-transactions";
/// Machine-local repository settings beside the shared metadata.
pub(crate) const LOCAL_REPO_CONFIG_FILE: &str = "local_repo_config.json";

#[derive(Debug, Clone)]
pub struct RepoPaths {
    pub(crate) repo_root: PathBuf,
    pub(crate) repo_metadata_path: PathBuf,
    pub(crate) local_repo_config_path: PathBuf,
    pub(crate) chunk_map_path: PathBuf,
    pub(crate) snapshots_root: PathBuf,
    pub(crate) bundles_root: PathBuf,
    pub(crate) chunks_root: PathBuf,
    /// File keys of an encrypted repository; `None` stores files as they are.
    pub(crate) sealing: Option<Arc<SealKeys>>,
}

impl RepoPaths {
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
            local_repo_config_path: repo_root.join(LOCAL_REPO_CONFIG_FILE),
            chunk_map_path: repo_root.join("chunk_map.gtcbin"),
            snapshots_root: repo_root.join("snapshots"),
            bundles_root: repo_root.join("bundles"),
            chunks_root: repo_root.join("chunks"),
            sealing: None,
        }
    }

    /// The layout of an existing repository, unlocked if it is encrypted.
    pub(crate) fn open(repo_root: &Path) -> Result<Self> {
        load_repo_manifest(repo_root).map(|(paths, _)| paths)
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Unlocks the layout with the keys of `manifest`, when it is encrypted.
    pub(crate) fn unlock_for(&mut self, manifest: &RepoManifest) -> Result<()> {
        self.sealing = match &manifest.encryption {
            None => None,
            Some(encryption) => Some(super::encryption::repo_keys(&self.repo_root, encryption)?),
        };
        Ok(())
    }

    /// The file keys, or `None` in a plain repository.
    pub(crate) fn keys(&self) -> Option<&SealKeys> {
        self.sealing.as_deref()
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
        return Err(Error::repo(format!(
            "invalid {label} path component '{value}'"
        )));
    }
    Ok(())
}

pub(crate) fn ensure_repo_layout(paths: &RepoPaths, name: &str) -> Result<RepoManifest> {
    filesystem::create_dir_all(&paths.snapshots_root)?;
    filesystem::create_dir_all(&paths.bundles_root)?;
    filesystem::create_dir_all(&paths.chunks_root)?;

    let repo_manifest = if paths.repo_metadata_path.exists() {
        read_repo_manifest(paths)?
    } else {
        let repo_manifest = RepoManifest::new(name.to_string());
        write_json(&paths.repo_metadata_path, &repo_manifest)?;
        repo_manifest
    };

    Ok(repo_manifest)
}

pub fn load_repo_manifest(repo_root: &Path) -> Result<(RepoPaths, RepoManifest)> {
    let mut paths = RepoPaths::from_repo_root(repo_root);
    if !paths.repo_metadata_path.exists() {
        return Err(Error::repo(format!(
            "gutch repo manifest not found at {}",
            paths.repo_metadata_path.display()
        )));
    }
    let repo_manifest = read_repo_manifest(&paths)?;
    paths.unlock_for(&repo_manifest)?;
    Ok((paths, repo_manifest))
}

fn read_repo_manifest(paths: &RepoPaths) -> Result<RepoManifest> {
    let mut manifest = RepoManifest::from_json_bytes(
        REPO_METADATA_FILE,
        &filesystem::read(&paths.repo_metadata_path)?,
    )?;
    apply_local_repo_config(paths, &mut manifest)?;
    Ok(manifest)
}

/// Refuse to change a repository whose format this release cannot write.
/// A missing or malformed manifest is left to the operation itself: one is
/// still being created, and forced initialization replaces the other.
pub(crate) fn ensure_repository_writable(repo_root: &Path) -> Result<()> {
    let path = repo_root.join(REPO_METADATA_FILE);
    if !path.exists() {
        return Ok(());
    }
    let bytes = filesystem::read(&path)?;
    let checked = check_document_markers::<RepoManifest>(REPO_METADATA_FILE, &bytes)
        .and_then(|()| RepoFormat::from_repo_metadata(&bytes)?.check(RepositoryAccess::Write));
    match checked {
        Err(error @ Error::UnsupportedFormat { .. }) => Err(error),
        _ => Ok(()),
    }
}

pub(crate) fn write_repo_manifest(paths: &RepoPaths, repo_manifest: &RepoManifest) -> Result<()> {
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
                document_type: DocumentType::default(),
                document_version: DocumentVersion::default(),
                remote_repo_id: remote_repo_id.clone(),
            },
        )?,
    }
    Ok(())
}

/// Machine-local repository settings, kept out of the shared metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalRepoConfig {
    document_type: DocumentType<LocalRepoConfig>,
    document_version: DocumentVersion<LocalRepoConfig>,
    remote_repo_id: String,
}

impl VersionedDocument for LocalRepoConfig {
    const DOCUMENT_TYPE: &'static str = "gutch_local_repo_config";
    const DOCUMENT_VERSION: &'static str = "1.0.0";
}

fn apply_local_repo_config(paths: &RepoPaths, manifest: &mut RepoManifest) -> Result<()> {
    if !paths.local_repo_config_path.exists() {
        return Ok(());
    }
    let config: LocalRepoConfig = parse_document(
        LOCAL_REPO_CONFIG_FILE,
        &filesystem::read(&paths.local_repo_config_path)?,
    )?;
    let remote_repo_id = config.remote_repo_id.trim();
    if remote_repo_id.is_empty() {
        return Err(Error::repo(format!(
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

/// Loose chunks are sharded by the first two hex digits of their file name.
pub(crate) fn chunk_path_in_root(chunks_root: &Path, file_name: ChunkHash) -> PathBuf {
    let hex = file_name.to_hex();
    chunks_root.join(&hex[..2]).join(hex)
}

pub(crate) fn chunk_path(paths: &RepoPaths, chunk_hash: ChunkHash) -> PathBuf {
    chunk_path_in_root(
        &paths.chunks_root,
        loose_chunk_file_name(paths.keys(), chunk_hash),
    )
}

/// A loose chunk's file name: its hash, or in an encrypted repository a keyed
/// hash of it, so stored names do not reveal which content is present.
pub(crate) fn loose_chunk_file_name(keys: Option<&SealKeys>, chunk_hash: ChunkHash) -> ChunkHash {
    match keys {
        None => chunk_hash,
        Some(keys) => ChunkHash::from_blake3(
            keys.name_hasher()
                .update(b"loose chunk")
                .update(chunk_hash.as_bytes())
                .finalize(),
        ),
    }
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
        assert!(bundle_path_in_root(store_root, "..\\escape").is_err());
        assert!(validate_path_component("C:\\escape", "component").is_err());
    }

    #[test]
    fn accepts_single_component_identifiers() {
        let paths = RepoPaths::from_repo_root(Path::new("repo-1"));
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
