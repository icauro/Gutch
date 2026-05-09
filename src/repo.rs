use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::manifest;
use crate::{Error, Result};

pub fn compute_target_id(
    version_id: &str,
    platform: &str,
    architecture: &str,
    tier_id: &str,
) -> String {
    format!("{version_id}-{platform}-{architecture}-{tier_id}")
}

pub fn canonical_target_tags(
    version_id: &str,
    platform: &str,
    architecture: &str,
    tier_id: &str,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("version".to_string(), version_id.to_string()),
        ("platform".to_string(), platform.to_string()),
        ("arch".to_string(), architecture.to_string()),
        ("tier".to_string(), tier_id.to_string()),
    ])
}

#[derive(Debug, Clone)]
pub struct ContentRepoPaths {
    pub repo_root: PathBuf,
    pub repo_manifest_path: PathBuf,
    pub targets_root: PathBuf,
    pub bundles_root: PathBuf,
}

impl ContentRepoPaths {
    pub fn from_store_root(store_root: &Path, product_id: &str, content_repo_id: &str) -> Self {
        let repo_root = store_root
            .join("v1")
            .join("products")
            .join(product_id)
            .join("repos")
            .join(content_repo_id);
        Self::from_repo_root(&repo_root)
    }

    pub fn from_repo_root(repo_root: &Path) -> Self {
        Self {
            repo_root: repo_root.to_path_buf(),
            repo_manifest_path: repo_root.join(manifest::CONTENT_REPO_MANIFEST_FILE),
            targets_root: repo_root.join("targets"),
            bundles_root: repo_root.join("bundles"),
        }
    }
}

pub fn ensure_content_repo_layout(
    paths: &ContentRepoPaths,
    product_id: &str,
    content_repo_id: &str,
) -> Result<manifest::ContentRepoManifest> {
    std::fs::create_dir_all(&paths.targets_root)?;
    std::fs::create_dir_all(&paths.bundles_root)?;

    let repo_manifest = if paths.repo_manifest_path.exists() {
        let repo_manifest_json = std::fs::read_to_string(&paths.repo_manifest_path)?;
        let repo_manifest: manifest::ContentRepoManifest =
            serde_json::from_str(&repo_manifest_json)?;
        if repo_manifest.product_id != product_id {
            return Err(Error::manifest(format!(
                "content repo at {} belongs to product '{}' not '{}'",
                paths.repo_root.display(),
                repo_manifest.product_id,
                product_id
            )));
        }
        if repo_manifest.content_repo_id != content_repo_id {
            return Err(Error::manifest(format!(
                "content repo at {} has repo id '{}' not '{}'",
                paths.repo_root.display(),
                repo_manifest.content_repo_id,
                content_repo_id
            )));
        }
        repo_manifest
    } else {
        let repo_manifest =
            manifest::ContentRepoManifest::new(product_id.to_string(), content_repo_id.to_string());
        let repo_manifest_json = serde_json::to_string_pretty(&repo_manifest)?;
        std::fs::write(&paths.repo_manifest_path, repo_manifest_json)?;
        repo_manifest
    };

    Ok(repo_manifest)
}

pub fn load_content_repo_manifest(
    repo_root: &Path,
) -> Result<(ContentRepoPaths, manifest::ContentRepoManifest)> {
    let paths = ContentRepoPaths::from_repo_root(repo_root);
    if !paths.repo_manifest_path.exists() {
        return Err(Error::manifest(format!(
            "content repo manifest not found at {}",
            paths.repo_manifest_path.display()
        )));
    }

    let repo_manifest_json = std::fs::read_to_string(&paths.repo_manifest_path)?;
    let repo_manifest: manifest::ContentRepoManifest = serde_json::from_str(&repo_manifest_json)?;

    if repo_manifest.targets_dir != "targets" || repo_manifest.bundles_dir != "bundles" {
        return Err(Error::manifest(format!(
            "unsupported content repo layout in {}",
            paths.repo_manifest_path.display()
        )));
    }

    Ok((paths, repo_manifest))
}

pub fn target_manifest_path(paths: &ContentRepoPaths, target_id: &str) -> PathBuf {
    paths.targets_root.join(target_id).join("manifest.json")
}
