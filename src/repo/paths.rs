use std::path::{Path, PathBuf};

use crate::manifest::REPO_METADATA_FILE;
use crate::manifest::gutch_repo::GutchRepoManifest;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct GutchRepoPaths {
    pub repo_root: PathBuf,
    pub repo_metadata_path: PathBuf,
    pub repo_binding_path: PathBuf,
    pub chunk_map_path: PathBuf,
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
            repo_metadata_path: repo_root.join(REPO_METADATA_FILE),
            repo_binding_path: repo_root.join("repo_binding.json"),
            chunk_map_path: repo_root.join("chunk_map.bin"),
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

    let repo_manifest = if paths.repo_metadata_path.exists() {
        let repo_manifest_json = crate::helpers::read_to_string(&paths.repo_metadata_path)?;
        let mut manifest: GutchRepoManifest = crate::helpers::from_json_str(&repo_manifest_json)?;
        apply_repo_binding(paths, &mut manifest)?;
        manifest
    } else {
        let repo_manifest = GutchRepoManifest::new(name.to_string());
        crate::helpers::write_json(&paths.repo_metadata_path, &repo_manifest)?;
        repo_manifest
    };

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
    apply_repo_binding(&paths, &mut repo_manifest)?;
    Ok((paths, repo_manifest))
}

pub fn write_gutch_repo_manifest(
    paths: &GutchRepoPaths,
    repo_manifest: &GutchRepoManifest,
) -> Result<()> {
    crate::helpers::write_json(&paths.repo_metadata_path, repo_manifest)?;
    if repo_manifest.remote_repo_id.is_empty() && repo_manifest.product_id.is_empty() {
        if paths.repo_binding_path.exists() {
            std::fs::remove_file(&paths.repo_binding_path)?;
        }
    } else {
        crate::helpers::write_json(
            &paths.repo_binding_path,
            &serde_json::json!({
                "product_id": repo_manifest.product_id,
                "remote_repo_id": repo_manifest.remote_repo_id,
            }),
        )?;
    }
    Ok(())
}

fn apply_repo_binding(paths: &GutchRepoPaths, manifest: &mut GutchRepoManifest) -> Result<()> {
    if !paths.repo_binding_path.exists() {
        return Ok(());
    }
    let binding: serde_json::Value =
        crate::helpers::from_json_str(&crate::helpers::read_to_string(&paths.repo_binding_path)?)?;
    manifest.product_id = binding
        .get("product_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    manifest.remote_repo_id = binding
        .get("remote_repo_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(())
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
