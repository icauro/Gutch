use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::manifest;
use crate::manifest::CONTENT_REPO_MANIFEST_FILE;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct ContentRepoPaths {
    pub repo_root: PathBuf,
    pub repo_manifest_path: PathBuf,
    pub chunk_index_path: PathBuf,
    pub targets_root: PathBuf,
    pub bundles_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInspectStatus {
    pub exists: bool,
    pub is_repo: bool,
    pub repo_path: String,
    pub product_id: String,
    pub content_repo_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_schema: Option<manifest::TargetTagSchema>,
    pub targets: Vec<RepoInspectTargetStatus>,
    pub bundle_count: usize,
    pub bundle_bytes: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInspectTargetStatus {
    pub target_id: String,
    pub version_id: String,
    pub platform: String,
    pub architecture: String,
    pub tier_id: String,
    pub file_count: usize,
    pub required_bundle_count: usize,
    pub total_logical_size: u64,
    pub manifest_path: String,
    pub has_signature_file: bool,
    pub missing_bundle_count: usize,
    pub target_tags: manifest::TargetTagSet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTargetInstallCandidate {
    pub path: String,
    pub folder_name: String,
    pub target_id: String,
    pub version_id: String,
    pub platform: String,
    pub architecture: String,
    pub tier_id: String,
    pub target_tags: manifest::TargetTagSet,
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
            repo_manifest_path: repo_root.join(CONTENT_REPO_MANIFEST_FILE),
            chunk_index_path: repo_root.join("chunk_index.json"),
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

pub fn load_chunk_index(paths: &ContentRepoPaths) -> Result<manifest::ChunkIndex> {
    if !paths.chunk_index_path.exists() {
        return Ok(manifest::ChunkIndex {
            format_version: 1,
            updated_at: String::new(),
            entries: BTreeMap::new(),
        });
    }
    let json = std::fs::read_to_string(&paths.chunk_index_path)?;
    let index: manifest::ChunkIndex = serde_json::from_str(&json)?;
    Ok(index)
}

pub fn write_chunk_index(paths: &ContentRepoPaths, index: &manifest::ChunkIndex) -> Result<()> {
    let json = serde_json::to_string_pretty(index)?;
    std::fs::write(&paths.chunk_index_path, json)?;
    Ok(())
}

pub fn target_manifest_path(paths: &ContentRepoPaths, target_id: &str) -> PathBuf {
    paths.targets_root.join(target_id).join("manifest.json")
}

pub fn inspect_content_repo(repo_root: &Path) -> Result<RepoInspectStatus> {
    let repo_path = repo_root.to_path_buf();
    let repo_path_display = repo_path.display().to_string();
    if !repo_path.exists() {
        return Ok(empty_repo_inspect_status(
            repo_path_display,
            false,
            Some("Path does not exist.".to_string()),
        ));
    }
    if !repo_path.is_dir() {
        return Ok(empty_repo_inspect_status(
            repo_path_display,
            true,
            Some("Path is not a directory.".to_string()),
        ));
    }

    let repo_manifest_path = repo_path.join(CONTENT_REPO_MANIFEST_FILE);
    if !repo_manifest_path.exists() {
        return Ok(empty_repo_inspect_status(
            repo_path_display,
            true,
            Some("repo_manifest.json was not found.".to_string()),
        ));
    }

    let (repo_paths, repo_manifest) = load_content_repo_manifest(&repo_path)?;
    let (_bundle_ids, bundle_count, bundle_bytes) =
        inspect_bundle_inventory(&repo_paths.bundles_root)?;
    let targets = inspect_target_manifests(&repo_paths.targets_root, &repo_paths.bundles_root)?;

    Ok(RepoInspectStatus {
        exists: true,
        is_repo: true,
        repo_path: repo_path_display,
        product_id: repo_manifest.product_id,
        content_repo_id: repo_manifest.content_repo_id,
        target_schema: repo_manifest.target_schema,
        targets,
        bundle_count,
        bundle_bytes,
        error: None,
    })
}

pub fn discover_source_target_installs(
    repo_root: &Path,
    source_targets_root: &Path,
) -> Result<Vec<SourceTargetInstallCandidate>> {
    if !source_targets_root.exists() {
        return Err(Error::manifest(format!(
            "source targets path does not exist: {}",
            source_targets_root.display()
        )));
    }
    if !source_targets_root.is_dir() {
        return Err(Error::manifest(format!(
            "source targets path is not a directory: {}",
            source_targets_root.display()
        )));
    }

    let (_repo_paths, repo_manifest) = load_content_repo_manifest(repo_root)?;
    let rules = repo_manifest.target_schema.unwrap_or_default();
    let mut candidates = Vec::new();

    for entry in std::fs::read_dir(source_targets_root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(candidate) = infer_source_target_candidate(&path, &rules) else {
            continue;
        };
        candidates.push(candidate);
    }

    candidates.sort_by(|left, right| {
        left.target_id
            .cmp(&right.target_id)
            .then(left.path.cmp(&right.path))
    });
    Ok(candidates)
}

fn empty_repo_inspect_status(
    repo_path: String,
    exists: bool,
    error: Option<String>,
) -> RepoInspectStatus {
    RepoInspectStatus {
        exists,
        is_repo: false,
        repo_path,
        product_id: String::new(),
        content_repo_id: String::new(),
        target_schema: None,
        targets: Vec::new(),
        bundle_count: 0,
        bundle_bytes: 0,
        error,
    }
}

fn inspect_bundle_inventory(bundles_root: &Path) -> Result<(HashSet<String>, usize, u64)> {
    if !bundles_root.exists() {
        return Ok((HashSet::new(), 0, 0));
    }

    let mut bundle_ids = HashSet::new();
    let mut bundle_count = 0usize;
    let mut bundle_bytes = 0u64;
    for entry in walkdir::WalkDir::new(bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|value| value.to_str()) != Some("bundle") {
            continue;
        }

        bundle_count += 1;
        bundle_bytes += entry.metadata()?.len();
        if let Some(bundle_id) = entry.path().file_stem().and_then(|value| value.to_str()) {
            bundle_ids.insert(bundle_id.to_string());
        }
    }

    Ok((bundle_ids, bundle_count, bundle_bytes))
}

fn inspect_target_manifests(
    targets_root: &Path,
    bundles_root: &Path,
) -> Result<Vec<RepoInspectTargetStatus>> {
    if !targets_root.exists() {
        return Ok(Vec::new());
    }

    let (available_bundle_ids, _, _) = inspect_bundle_inventory(bundles_root)?;
    let mut targets = Vec::new();
    for entry in std::fs::read_dir(targets_root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }

        let manifest_json = std::fs::read_to_string(&manifest_path)?;
        let manifest: manifest::TargetManifest = serde_json::from_str(&manifest_json)?;
        let required_bundle_ids = required_bundle_ids(&manifest);
        let missing_bundle_count = required_bundle_ids
            .iter()
            .filter(|bundle_id| !available_bundle_ids.contains(*bundle_id))
            .count();

        targets.push(RepoInspectTargetStatus {
            target_id: manifest.target_id.clone(),
            version_id: exclusive_tag_or_empty(&manifest.target_tags, "version"),
            platform: exclusive_tag_or_empty(&manifest.target_tags, "os"),
            architecture: exclusive_tag_or_empty(&manifest.target_tags, "arch"),
            tier_id: exclusive_tag_or_empty(&manifest.target_tags, "tier"),
            file_count: manifest.files.len(),
            required_bundle_count: required_bundle_ids.len(),
            total_logical_size: manifest.total_logical_size,
            manifest_path: manifest_path.display().to_string(),
            has_signature_file: path.join("manifest.sig").exists(),
            missing_bundle_count,
            target_tags: manifest.target_tags,
        });
    }

    targets.sort_by(|left, right| left.target_id.cmp(&right.target_id));
    Ok(targets)
}

fn infer_source_target_candidate(
    path: &Path,
    rules: &manifest::TargetTagSchema,
) -> Option<SourceTargetInstallCandidate> {
    use crate::cli_identity::resolve_target_identity;

    let context = format!("source target '{}'", path.display());
    let resolved = resolve_target_identity(path, &manifest::TargetTagSet::default(), rules, &context).ok()?;

    let folder_name = path.file_name()?.to_str()?.to_string();

    Some(SourceTargetInstallCandidate {
        path: path.display().to_string(),
        folder_name,
        target_id: resolved.target_id,
        version_id: exclusive_tag_or_empty(&resolved.target_tags, "version"),
        platform: exclusive_tag_or_empty(&resolved.target_tags, "os"),
        architecture: exclusive_tag_or_empty(&resolved.target_tags, "arch"),
        tier_id: exclusive_tag_or_empty(&resolved.target_tags, "tier"),
        target_tags: resolved.target_tags,
    })
}

fn exclusive_tag_or_empty(target_tags: &manifest::TargetTagSet, prefix: &str) -> String {
    target_tags.get_exclusive(prefix).unwrap_or_default().to_string()
}

fn required_bundle_ids(target_manifest: &manifest::TargetManifest) -> Vec<String> {
    target_manifest
        .files
        .iter()
        .flat_map(|file| file.chunks.iter().map(|chunk| chunk.chunk_hash.clone()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exclusive_tags(version: &str, os: &str, arch: &str, tier: &str) -> manifest::TargetTagSet {
        let mut tags = manifest::TargetTagSet::default();
        tags.upsert_exclusive("version", version);
        tags.upsert_exclusive("os", os);
        tags.upsert_exclusive("arch", arch);
        tags.upsert_exclusive("tier", tier);
        tags
    }

    #[test]
    fn content_repo_paths_from_repo_root() {
        let root = PathBuf::from("/test/repo");
        let paths = ContentRepoPaths::from_repo_root(&root);
        assert_eq!(paths.repo_root, root);
        assert_eq!(paths.repo_manifest_path, root.join("repo_manifest.json"));
        assert_eq!(paths.chunk_index_path, root.join("chunk_index.json"));
        assert_eq!(paths.targets_root, root.join("targets"));
        assert_eq!(paths.bundles_root, root.join("bundles"));
    }

    #[test]
    fn content_repo_paths_from_store_root() {
        let root = PathBuf::from("/test/store");
        let paths = ContentRepoPaths::from_store_root(&root, "p1", "my-repo");
        assert!(paths.repo_root.to_string_lossy().contains("my-repo"));
        assert!(paths.repo_manifest_path.to_string_lossy().contains("my-repo"));
    }

    #[test]
    fn inspect_content_repo_reports_targets() {
        let root = std::env::temp_dir().join(format!(
            "gutch_repo_inspect_test_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let repo_root = root.join("repo");
        let repo_paths = ContentRepoPaths::from_repo_root(&repo_root);
        let _ = std::fs::remove_dir_all(&root);

        ensure_content_repo_layout(&repo_paths, "product-1", "base").unwrap();
        let target_root = repo_paths.targets_root.join("abc123def");
        std::fs::create_dir_all(&target_root).unwrap();
        let mut manifest = manifest::TargetManifest::new(
            "product-1".to_string(),
            "base".to_string(),
            "abc123def".to_string(),
            exclusive_tags("1.0.0", "windows", "x86_64", "base"),
        );
        manifest.files.push(manifest::FileManifest {
            path: "game.exe".to_string(),
            size: 42,
            mode: 0o644,
            executable: true,
            symlink_target: None,
            full_file_hash: "abc123".to_string(),
            chunks: Vec::new(),
        });

        std::fs::write(
            target_root.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(target_root.join("manifest.sig"), b"sig").unwrap();
        std::fs::write(repo_paths.bundles_root.join("bundle-a.bundle"), b"bundle-a").unwrap();

        let status = inspect_content_repo(&repo_root).unwrap();

        assert!(status.exists);
        assert!(status.is_repo);
        assert_eq!(status.product_id, "product-1");
        assert_eq!(status.content_repo_id, "base");
        assert_eq!(status.bundle_count, 1);
        assert_eq!(status.targets.len(), 1);
        assert_eq!(status.targets[0].target_id, "abc123def");
        assert!(status.targets[0].has_signature_file);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn discover_source_target_installs_returns_matching_subfolders_only() {
        let root = std::env::temp_dir().join(format!(
            "gutch_source_discover_test_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let repo_root = root.join("repo");
        let source_root = root.join("source-targets");
        let repo_paths = ContentRepoPaths::from_repo_root(&repo_root);
        let _ = std::fs::remove_dir_all(&root);

        ensure_content_repo_layout(&repo_paths, "", "mods").unwrap();

        let repo_manifest = manifest::ContentRepoManifest {
            target_schema: Some(manifest::TargetTagSchema {
                target_tags: vec![
                    manifest::TargetTagDefinition {
                        name: "version".to_string(),
                        kind: manifest::TargetTagKind::Exclusive,
                        required: true,
                        values: Vec::new(),
                        aliases: BTreeMap::new(),
                        default_value: None,
                    },
                    manifest::TargetTagDefinition {
                        name: "loader".to_string(),
                        kind: manifest::TargetTagKind::Scoped,
                        required: false,
                        values: Vec::new(),
                        aliases: BTreeMap::from([
                            ("fabric".to_string(), "fabric".to_string()),
                            ("forge".to_string(), "forge".to_string()),
                            ("neoforge".to_string(), "neoforge".to_string()),
                        ]),
                        default_value: None,
                    },
                    manifest::TargetTagDefinition {
                        name: "mc".to_string(),
                        kind: manifest::TargetTagKind::Scoped,
                        required: false,
                        values: Vec::new(),
                        aliases: BTreeMap::from([
                            ("1.20.1".to_string(), "1.20.1".to_string()),
                            ("1.21".to_string(), "1.21".to_string()),
                        ]),
                        default_value: None,
                    },
                ],
                target_id_order: vec!["version".to_string(), "loader".to_string(), "mc".to_string()],
                on_incomplete: manifest::IncompleteInferenceBehavior::Warn,
            }),
            plugin: None,
            ..manifest::ContentRepoManifest::new("".to_string(), "mods".to_string())
        };
        std::fs::write(
            &repo_paths.repo_manifest_path,
            serde_json::to_string_pretty(&repo_manifest).unwrap(),
        )
        .unwrap();

        std::fs::create_dir_all(source_root.join("ae2-v15.4.10-fabric-1.21")).unwrap();
        std::fs::create_dir_all(source_root.join("not-a-target-folder")).unwrap();

        let candidates = discover_source_target_installs(&repo_root, &source_root).unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].folder_name, "ae2-v15.4.10-fabric-1.21");
        assert_eq!(candidates[0].version_id, "15.4.10");
        assert_eq!(
            candidates[0]
                .target_tags
                .scoped
                .get("loader")
                .and_then(|values| values.iter().next())
                .map(String::as_str),
            Some("fabric")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn chunk_index_write_and_load_roundtrip() {
        let root = std::env::temp_dir().join("gutch_chunk_index_test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = ContentRepoPaths::from_repo_root(&root);

        let mut entries = BTreeMap::new();
        entries.insert("chunk-a".to_string(), "bundle-1".to_string());
        entries.insert("chunk-b".to_string(), "bundle-1".to_string());
        entries.insert("chunk-c".to_string(), "bundle-2".to_string());
        let index = manifest::ChunkIndex {
            format_version: 1,
            updated_at: "2026-05-14T00:00:00Z".to_string(),
            entries,
        };
        write_chunk_index(&paths, &index).unwrap();
        assert!(paths.chunk_index_path.exists());

        let loaded = load_chunk_index(&paths).unwrap();
        assert_eq!(loaded.format_version, 1);
        assert_eq!(loaded.entries.len(), 3);
        assert_eq!(loaded.entries.get("chunk-a").map(String::as_str), Some("bundle-1"));
        assert_eq!(loaded.entries.get("chunk-c").map(String::as_str), Some("bundle-2"));
        assert!(loaded.entries.get("nonexistent").is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn chunk_index_load_empty_when_file_missing() {
        let root = std::env::temp_dir().join("gutch_chunk_index_empty_test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = ContentRepoPaths::from_repo_root(&root);

        let loaded = load_chunk_index(&paths).unwrap();
        assert!(loaded.entries.is_empty());
        assert_eq!(loaded.format_version, 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn inspect_repo_shows_target_tags() {
        let root = std::env::temp_dir().join("gutch_inspect_tags_test");
        let _ = std::fs::remove_dir_all(&root);
        let paths = ContentRepoPaths::from_repo_root(&root);
        ensure_content_repo_layout(&paths, "product-tags", "base").unwrap();

        let target_root = paths.targets_root.join("hash123");
        std::fs::create_dir_all(&target_root).unwrap();
        let mut manifest = manifest::TargetManifest::new(
            "product-tags".to_string(),
            "base".to_string(),
            "hash123".to_string(),
            exclusive_tags("2.0.0", "linux", "x86_64", "premium"),
        );
        manifest.files.push(manifest::FileManifest {
            path: "game.bin".to_string(),
            size: 100,
            mode: 0o755,
            executable: true,
            symlink_target: None,
            full_file_hash: "abc".to_string(),
            chunks: vec![],
        });
        std::fs::write(
            target_root.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        ).unwrap();

        let status = inspect_content_repo(&root).unwrap();
        assert_eq!(status.targets.len(), 1);
        assert_eq!(status.targets[0].target_id, "hash123");
        assert_eq!(status.targets[0].version_id, "2.0.0");
        assert_eq!(status.targets[0].platform, "linux");

        let _ = std::fs::remove_dir_all(&root);
    }
}


