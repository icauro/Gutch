use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::manifest;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInspectStatus {
    pub exists: bool,
    pub is_repo: bool,
    pub repo_path: String,
    pub product_id: String,
    pub content_repo_id: String,
    pub target_schema: Option<manifest::ContentRepoTargetSchema>,
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
    pub target_tags: BTreeMap<String, String>,
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
    pub target_tags: BTreeMap<String, String>,
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

    let repo_manifest_path = repo_path.join(manifest::CONTENT_REPO_MANIFEST_FILE);
    if !repo_manifest_path.exists() {
        return Ok(empty_repo_inspect_status(
            repo_path_display,
            true,
            Some("repo.json was not found.".to_string()),
        ));
    }

    let (repo_paths, repo_manifest) = load_content_repo_manifest(&repo_path)?;
    let (bundle_ids, bundle_count, bundle_bytes) =
        inspect_bundle_inventory(&repo_paths.bundles_root)?;
    let targets = inspect_target_manifests(&repo_paths.targets_root, &bundle_ids)?;

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
    let schema = effective_target_schema(repo_manifest.target_schema);
    let mut candidates = Vec::new();

    for entry in std::fs::read_dir(source_targets_root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(candidate) = infer_source_target_candidate(&path, &schema) else {
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
    bundle_ids: &HashSet<String>,
) -> Result<Vec<RepoInspectTargetStatus>> {
    if !targets_root.exists() {
        return Ok(Vec::new());
    }

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
        let missing_bundle_count = manifest
            .required_bundle_set
            .iter()
            .filter(|bundle_id| !bundle_ids.contains(bundle_id.as_str()))
            .count();

        targets.push(RepoInspectTargetStatus {
            target_id: manifest.target_id,
            version_id: manifest.version_id,
            platform: manifest.platform,
            architecture: manifest.architecture,
            tier_id: manifest.tier_id,
            file_count: manifest.files.len(),
            required_bundle_count: manifest.required_bundle_set.len(),
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct InferredTargetIdentity {
    version_id: Option<String>,
    platform: Option<String>,
    architecture: Option<String>,
}

const SUPPORTED_PLATFORM_ALIASES: &[(&str, &str)] = &[
    ("any", "any"),
    ("windows", "windows"),
    ("win", "windows"),
    ("win32", "windows"),
    ("win64", "windows"),
    ("linux", "linux"),
    ("mac", "macos"),
    ("macos", "macos"),
    ("osx", "macos"),
    ("darwin", "macos"),
];

const SUPPORTED_ARCHITECTURE_ALIASES: &[(&str, &str)] = &[
    ("any", "any"),
    ("x86_64", "x86_64"),
    ("x64", "x86_64"),
    ("amd64", "x86_64"),
    ("win64", "x86_64"),
    ("x86_32", "x86_32"),
    ("x86", "x86_32"),
    ("win32", "x86_32"),
];

fn default_target_schema() -> manifest::ContentRepoTargetSchema {
    manifest::ContentRepoTargetSchema {
        target_tags: vec![
            manifest::ContentRepoTargetTagDefinition {
                name: "platform".to_string(),
                required: true,
                values: vec![
                    "windows".to_string(),
                    "linux".to_string(),
                    "macos".to_string(),
                    "any".to_string(),
                ],
                aliases: BTreeMap::from([
                    ("win".to_string(), "windows".to_string()),
                    ("win32".to_string(), "windows".to_string()),
                    ("win64".to_string(), "windows".to_string()),
                    ("mac".to_string(), "macos".to_string()),
                    ("osx".to_string(), "macos".to_string()),
                    ("darwin".to_string(), "macos".to_string()),
                ]),
                default_value: Some("any".to_string()),
            },
            manifest::ContentRepoTargetTagDefinition {
                name: "arch".to_string(),
                required: true,
                values: vec![
                    "x86_64".to_string(),
                    "x86_32".to_string(),
                    "any".to_string(),
                ],
                aliases: BTreeMap::from([
                    ("x64".to_string(), "x86_64".to_string()),
                    ("amd64".to_string(), "x86_64".to_string()),
                    ("win64".to_string(), "x86_64".to_string()),
                    ("x86".to_string(), "x86_32".to_string()),
                    ("win32".to_string(), "x86_32".to_string()),
                ]),
                default_value: Some("any".to_string()),
            },
            manifest::ContentRepoTargetTagDefinition {
                name: "tier".to_string(),
                required: false,
                values: Vec::new(),
                aliases: BTreeMap::new(),
                default_value: Some("base".to_string()),
            },
        ],
        target_id_order: vec![
            "version".to_string(),
            "platform".to_string(),
            "arch".to_string(),
            "tier".to_string(),
        ],
    }
}

fn effective_target_schema(
    schema: Option<manifest::ContentRepoTargetSchema>,
) -> manifest::ContentRepoTargetSchema {
    schema.unwrap_or_else(default_target_schema)
}

fn infer_source_target_candidate(
    path: &Path,
    schema: &manifest::ContentRepoTargetSchema,
) -> Option<SourceTargetInstallCandidate> {
    let inferred = infer_target_identity_from_path(path);
    let inferred_custom_tags = infer_custom_tag_values_from_path(path, schema);
    let version_id = inferred.version_id.as_deref().and_then(normalize_version)?;

    let mut target_tags = BTreeMap::new();
    target_tags.insert("version".to_string(), version_id.clone());
    for definition in &schema.target_tags {
        let inferred_value = inferred_custom_tags
            .get(&definition.name)
            .map(String::as_str)
            .or_else(|| {
                if definition.name == "platform" {
                    inferred.platform.as_deref()
                } else if definition.name == "arch" {
                    inferred.architecture.as_deref()
                } else {
                    None
                }
            });

        let normalized_value = if let Some(value) = inferred_value {
            value.to_string()
        } else if let Some(default_value) = &definition.default_value {
            default_value.clone()
        } else if definition.required {
            return None;
        } else {
            continue;
        };
        target_tags.insert(definition.name.clone(), normalized_value);
    }

    let platform = target_tags
        .get("platform")
        .cloned()
        .unwrap_or_else(|| "any".to_string());
    let architecture = target_tags
        .get("arch")
        .cloned()
        .unwrap_or_else(|| "any".to_string());
    let tier_id = target_tags
        .get("tier")
        .cloned()
        .unwrap_or_else(|| "base".to_string());
    let folder_name = path.file_name()?.to_str()?.to_string();

    Some(SourceTargetInstallCandidate {
        path: path.display().to_string(),
        folder_name,
        target_id: build_target_id_from_tags(&version_id, &target_tags, schema),
        version_id,
        platform,
        architecture,
        tier_id,
        target_tags,
    })
}

fn normalize_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    if candidate.is_empty() {
        return None;
    }
    let parts: Vec<&str> = candidate.split('.').collect();
    if parts.is_empty()
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()))
    {
        return None;
    }
    Some(candidate.to_string())
}

fn normalize_platform(raw: &str) -> Option<String> {
    let lowered = raw.trim().to_ascii_lowercase();
    SUPPORTED_PLATFORM_ALIASES
        .iter()
        .find_map(|(alias, canonical)| (*alias == lowered).then(|| (*canonical).to_string()))
}

fn normalize_architecture(raw: &str) -> Option<String> {
    let lowered = raw.trim().to_ascii_lowercase();
    SUPPORTED_ARCHITECTURE_ALIASES
        .iter()
        .find_map(|(alias, canonical)| (*alias == lowered).then(|| (*canonical).to_string()))
}

fn infer_tag_value_from_token(
    token: &str,
    tag_definition: &manifest::ContentRepoTargetTagDefinition,
) -> Option<String> {
    let lowered = token.trim().to_ascii_lowercase();
    if let Some(value) = tag_definition.aliases.get(&lowered) {
        return Some(value.clone());
    }

    tag_definition
        .values
        .iter()
        .find(|value| value.eq_ignore_ascii_case(token))
        .cloned()
}

fn normalize_custom_tag_value(
    raw: &str,
    tag_definition: &manifest::ContentRepoTargetTagDefinition,
) -> Option<String> {
    if tag_definition.name == "platform" {
        return normalize_platform(raw);
    }
    if tag_definition.name == "arch" {
        return normalize_architecture(raw);
    }
    if tag_definition.name == "tier" && tag_definition.values.is_empty() {
        return Some(raw.trim().to_ascii_lowercase());
    }

    infer_tag_value_from_token(raw, tag_definition).or_else(|| {
        tag_definition
            .values
            .is_empty()
            .then(|| raw.trim().to_string())
    })
}

fn tokenize_identity_name(name: &str) -> Vec<String> {
    name.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.'))
        .filter(|token| !token.is_empty())
        .map(|token| token.to_string())
        .collect()
}

fn infer_target_identity_from_path(path: &Path) -> InferredTargetIdentity {
    let raw_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let mut inferred = InferredTargetIdentity::default();
    for token in tokenize_identity_name(raw_name) {
        if inferred.version_id.is_none() {
            inferred.version_id = normalize_version(&token);
        }
        if inferred.platform.is_none() {
            inferred.platform = normalize_platform(&token);
        }
        if inferred.architecture.is_none() {
            inferred.architecture = normalize_architecture(&token);
        }
    }
    inferred
}

fn infer_custom_tag_values_from_path(
    path: &Path,
    schema: &manifest::ContentRepoTargetSchema,
) -> BTreeMap<String, String> {
    let raw_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let mut inferred_tags = BTreeMap::new();
    for token in tokenize_identity_name(raw_name) {
        for definition in &schema.target_tags {
            if inferred_tags.contains_key(&definition.name) {
                continue;
            }
            if let Some(value) = normalize_custom_tag_value(&token, definition) {
                inferred_tags.insert(definition.name.clone(), value);
            }
        }
    }
    inferred_tags
}

fn build_target_id_from_tags(
    version_id: &str,
    target_tags: &BTreeMap<String, String>,
    schema: &manifest::ContentRepoTargetSchema,
) -> String {
    let mut parts = vec![version_id.to_string()];
    let order = if schema.target_id_order.is_empty() {
        schema
            .target_tags
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>()
    } else {
        schema
            .target_id_order
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    };

    for tag_name in order {
        if tag_name == "version" {
            continue;
        }
        if let Some(tag_value) = target_tags.get(tag_name) {
            parts.push(tag_value.clone());
        }
    }
    parts.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_content_repo_reports_targets_and_missing_bundles() {
        let root = std::env::temp_dir().join(format!(
            "gutch_repo_inspect_test_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let repo_root = root.join("repo");
        let repo_paths = ContentRepoPaths::from_repo_root(&repo_root);
        let _ = std::fs::remove_dir_all(&root);

        ensure_content_repo_layout(&repo_paths, "product-1", "base").unwrap();
        let target_root = repo_paths.targets_root.join("1.0.0-windows-x86_64-base");
        std::fs::create_dir_all(&target_root).unwrap();
        let mut manifest = manifest::TargetManifest::new(
            "product-1".to_string(),
            "base".to_string(),
            "1.0.0-windows-x86_64-base".to_string(),
            "1.0.0".to_string(),
            "windows".to_string(),
            "x86_64".to_string(),
            "base".to_string(),
            canonical_target_tags("1.0.0", "windows", "x86_64", "base"),
        );
        manifest.required_bundle_set = vec!["bundle-a".to_string(), "bundle-missing".to_string()];
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
        assert_eq!(status.targets[0].target_id, "1.0.0-windows-x86_64-base");
        assert_eq!(status.targets[0].missing_bundle_count, 1);
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
            target_schema: Some(manifest::ContentRepoTargetSchema {
                target_tags: vec![
                    manifest::ContentRepoTargetTagDefinition {
                        name: "loader".to_string(),
                        required: true,
                        values: vec!["fabric".to_string(), "forge".to_string()],
                        aliases: BTreeMap::new(),
                        default_value: None,
                    },
                    manifest::ContentRepoTargetTagDefinition {
                        name: "mc".to_string(),
                        required: true,
                        values: vec!["1.20.1".to_string(), "1.21".to_string()],
                        aliases: BTreeMap::new(),
                        default_value: None,
                    },
                ],
                target_id_order: vec![
                    "version".to_string(),
                    "loader".to_string(),
                    "mc".to_string(),
                ],
            }),
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
        assert_eq!(candidates[0].target_id, "15.4.10-fabric-1.21");
        assert_eq!(candidates[0].version_id, "15.4.10");

        let _ = std::fs::remove_dir_all(&root);
    }
}
