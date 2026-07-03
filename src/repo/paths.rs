use std::path::{Path, PathBuf};

use crate::manifest::CONTENT_REPO_MANIFEST_FILE;
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct GutchRepoPaths {
    pub repo_root: PathBuf,
    pub repo_manifest_path: PathBuf,
    pub chunk_index_path: PathBuf,
    pub targets_root: PathBuf,
    pub bundles_root: PathBuf,
    pub chunks_root: PathBuf,
}

impl GutchRepoPaths {
    pub fn from_store_root(store_root: &Path, gutch_repo_id: &str) -> Self {
        let repo_root = store_root.join("v1").join("repos").join(gutch_repo_id);
        Self::from_repo_root(&repo_root)
    }

    pub fn from_repo_root(repo_root: &Path) -> Self {
        Self {
            repo_root: repo_root.to_path_buf(),
            repo_manifest_path: repo_root.join(CONTENT_REPO_MANIFEST_FILE),
            chunk_index_path: repo_root.join("chunk_index.bin"),
            targets_root: repo_root.join("targets"),
            bundles_root: repo_root.join("bundles"),
            chunks_root: repo_root.join("chunks"),
        }
    }
}

pub fn ensure_gutch_repo_layout(paths: &GutchRepoPaths, name: &str) -> Result<GutchRepoManifest> {
    crate::helpers::create_dir_all(&paths.targets_root)?;
    crate::helpers::create_dir_all(&paths.bundles_root)?;
    crate::helpers::create_dir_all(&paths.chunks_root)?;

    let repo_manifest = if paths.repo_manifest_path.exists() {
        let repo_manifest_json = crate::helpers::read_to_string(&paths.repo_manifest_path)?;
        crate::helpers::from_json_str(&repo_manifest_json)?
    } else {
        let repo_manifest = GutchRepoManifest::new(name.to_string());
        crate::helpers::write_json(&paths.repo_manifest_path, &repo_manifest)?;
        repo_manifest
    };

    Ok(repo_manifest)
}

pub fn load_gutch_repo_manifest(repo_root: &Path) -> Result<(GutchRepoPaths, GutchRepoManifest)> {
    let paths = GutchRepoPaths::from_repo_root(repo_root);
    if !paths.repo_manifest_path.exists() {
        return Err(Error::manifest(format!(
            "gutch repo manifest not found at {}",
            paths.repo_manifest_path.display()
        )));
    }

    let repo_manifest_json = crate::helpers::read_to_string(&paths.repo_manifest_path)?;
    let repo_manifest: GutchRepoManifest = crate::helpers::from_json_str(&repo_manifest_json)?;
    Ok((paths, repo_manifest))
}

pub fn write_gutch_repo_manifest(
    paths: &GutchRepoPaths,
    repo_manifest: &GutchRepoManifest,
) -> Result<()> {
    crate::helpers::write_json(&paths.repo_manifest_path, repo_manifest)
}

pub fn bundle_path_in_root(bundles_root: &Path, bundle_id: &str) -> PathBuf {
    bundles_root.join(bundle_id)
}

pub fn bundle_path(paths: &GutchRepoPaths, bundle_id: &str) -> PathBuf {
    bundle_path_in_root(&paths.bundles_root, bundle_id)
}

pub fn chunk_path_in_root(chunks_root: &Path, chunk_hash: &str) -> PathBuf {
    let prefix = chunk_hash.get(0..2).unwrap_or("xx");
    chunks_root.join(prefix).join(format!("{chunk_hash}.chunk"))
}

pub fn chunk_path(paths: &GutchRepoPaths, chunk_hash: &str) -> PathBuf {
    chunk_path_in_root(&paths.chunks_root, chunk_hash)
}

pub fn bundle_id_from_bundle_path(path: &Path) -> Option<String> {
    if path.extension().and_then(|value| value.to_str()) == Some("bundle") {
        return path
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
    }

    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && !value.contains('.'))
        .map(ToString::to_string)
}
