use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manifest::REPO_METADATA_FILE;
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct GutchRepoPaths {
    pub(crate) repo_root: PathBuf,
    pub(crate) repo_metadata_path: PathBuf,
    pub(crate) local_repo_config_path: PathBuf,
    pub(crate) chunk_map_path: PathBuf,
    pub(crate) snapshots_root: PathBuf,
    pub(crate) bundles_root: PathBuf,
    pub(crate) chunks_root: PathBuf,
}

impl GutchRepoPaths {
    pub fn from_store_root(store_root: &Path, gutch_repo_id: &str) -> Result<Self> {
        validate_path_component(gutch_repo_id, "Gutch repo ID")?;
        let repo_root = store_root.join("v1").join("repos").join(gutch_repo_id);
        Ok(Self::from_repo_root(&repo_root))
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
        return Err(Error::manifest(format!(
            "invalid {label} path component '{value}'"
        )));
    }
    Ok(())
}

pub(crate) fn ensure_gutch_repo_layout(
    paths: &GutchRepoPaths,
    name: &str,
) -> Result<GutchRepoManifest> {
    crate::helpers::create_dir_all(&paths.snapshots_root)?;
    crate::helpers::create_dir_all(&paths.bundles_root)?;
    crate::helpers::create_dir_all(&paths.chunks_root)?;

    let repo_manifest = if paths.repo_metadata_path.exists() {
        let repo_manifest_json = crate::helpers::read_to_string(&paths.repo_metadata_path)?;
        let mut manifest: GutchRepoManifest = crate::helpers::from_json_str(&repo_manifest_json)?;
        apply_local_repo_config(paths, &mut manifest)?;
        manifest
    } else {
        let repo_manifest = GutchRepoManifest::new(name.to_string());
        crate::helpers::write_json(&paths.repo_metadata_path, &repo_manifest)?;
        repo_manifest
    };

    validate_repo_snapshot_tag_schema(&repo_manifest)?;
    Ok(repo_manifest)
}

pub fn load_gutch_repo_manifest(repo_root: &Path) -> Result<(GutchRepoPaths, GutchRepoManifest)> {
    let paths = GutchRepoPaths::from_repo_root(repo_root);
    if !paths.repo_metadata_path.exists() {
        return Err(Error::manifest(format!(
            "gutch repo manifest not found at {}",
            paths.repo_metadata_path.display()
        )));
    }
    let repo_manifest_json = crate::helpers::read_to_string(&paths.repo_metadata_path)?;
    let mut repo_manifest: GutchRepoManifest = crate::helpers::from_json_str(&repo_manifest_json)?;
    validate_repo_snapshot_tag_schema(&repo_manifest)?;
    apply_local_repo_config(&paths, &mut repo_manifest)?;
    Ok((paths, repo_manifest))
}

pub(crate) fn write_gutch_repo_manifest(
    paths: &GutchRepoPaths,
    repo_manifest: &GutchRepoManifest,
) -> Result<()> {
    validate_repo_snapshot_tag_schema(repo_manifest)?;
    crate::helpers::write_json(&paths.repo_metadata_path, repo_manifest)?;
    if repo_manifest.remote_repo_id.is_empty() {
        if paths.local_repo_config_path.exists() {
            std::fs::remove_file(&paths.local_repo_config_path)?;
        }
    } else {
        crate::helpers::write_json(
            &paths.local_repo_config_path,
            &LocalRepoConfig {
                remote_repo_id: repo_manifest.remote_repo_id.clone(),
            },
        )?;
    }
    Ok(())
}

fn validate_repo_snapshot_tag_schema(repo_manifest: &GutchRepoManifest) -> Result<()> {
    let Some(schema) = repo_manifest.snapshot_tag_schema.as_ref() else {
        return Ok(());
    };
    let validation = crate::manifest::validate_snapshot_tag_schema(schema);
    if let Some(error) = validation.errors.first() {
        return Err(Error::manifest(format!(
            "invalid snapshot tag schema at {}: {}",
            error.field, error.message
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalRepoConfig {
    remote_repo_id: String,
}

fn apply_local_repo_config(paths: &GutchRepoPaths, manifest: &mut GutchRepoManifest) -> Result<()> {
    if !paths.local_repo_config_path.exists() {
        return Ok(());
    }
    let config: LocalRepoConfig = crate::helpers::from_json_str(&crate::helpers::read_to_string(
        &paths.local_repo_config_path,
    )?)?;
    manifest.remote_repo_id = config.remote_repo_id;
    Ok(())
}

pub(crate) fn bundle_path_in_root(bundles_root: &Path, bundle_id: &str) -> Result<PathBuf> {
    validate_path_component(bundle_id, "bundle ID")?;
    Ok(bundles_root.join(bundle_id))
}

pub(crate) fn bundle_path(paths: &GutchRepoPaths, bundle_id: &str) -> Result<PathBuf> {
    bundle_path_in_root(&paths.bundles_root, bundle_id)
}

pub(crate) fn chunk_path_in_root(chunks_root: &Path, chunk_hash: &str) -> Result<PathBuf> {
    validate_path_component(chunk_hash, "chunk hash")?;
    let prefix = chunk_hash.get(0..2).unwrap_or("xx");
    Ok(chunks_root.join(prefix).join(chunk_hash))
}

pub(crate) fn chunk_path(paths: &GutchRepoPaths, chunk_hash: &str) -> Result<PathBuf> {
    chunk_path_in_root(&paths.chunks_root, chunk_hash)
}

pub(crate) fn bundle_id_from_bundle_path(path: &Path) -> Option<String> {
    let bundle_id = path.file_name().and_then(|value| value.to_str())?;
    crate::ChunkHash::parse_hex(bundle_id).ok()?;
    Some(bundle_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_components() {
        let store_root = Path::new("store");
        assert!(GutchRepoPaths::from_store_root(store_root, "../escape").is_err());
        assert!(bundle_path_in_root(store_root, "..\\escape").is_err());
        assert!(chunk_path_in_root(store_root, "nested/hash").is_err());
        assert!(validate_path_component("C:\\escape", "component").is_err());
    }

    #[test]
    fn accepts_single_component_identifiers() {
        let paths = GutchRepoPaths::from_store_root(Path::new("store"), "repo-1").unwrap();
        assert!(paths.repo_root().ends_with(Path::new("v1/repos/repo-1")));
        assert!(bundle_path(&paths, "bundle-1").is_ok());
        assert!(chunk_path(&paths, "ab123").is_ok());
    }
}
