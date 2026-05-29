use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::hash::hash_hex;
use crate::manifest::CONTENT_REPO_MANIFEST_FILE;
use crate::manifest::{
    self, FileManifest, content_repo::ContentRepoManifest, target::TargetManifest,
};
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

#[derive(Debug, Default)]
pub struct ExistingStoreState {
    pub bundle_ids: HashSet<String>,
    pub chunk_locations: std::collections::HashMap<String, crate::ExistingChunkLocation>,
    pub previous_file_manifests: std::collections::HashMap<String, FileManifest>,
}

impl ContentRepoPaths {
    pub fn from_store_root(store_root: &Path, content_repo_id: &str) -> Self {
        let repo_root = store_root.join("v1").join("repos").join(content_repo_id);
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
    content_repo_id: &str,
) -> Result<ContentRepoManifest> {
    std::fs::create_dir_all(&paths.targets_root)?;
    std::fs::create_dir_all(&paths.bundles_root)?;

    let repo_manifest = if paths.repo_manifest_path.exists() {
        let repo_manifest_json = std::fs::read_to_string(&paths.repo_manifest_path)?;
        let repo_manifest: ContentRepoManifest = serde_json::from_str(&repo_manifest_json)?;
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
        let repo_manifest = ContentRepoManifest::new(content_repo_id.to_string());
        let repo_manifest_json = serde_json::to_string_pretty(&repo_manifest)?;
        std::fs::write(&paths.repo_manifest_path, repo_manifest_json)?;
        repo_manifest
    };

    Ok(repo_manifest)
}

pub fn load_content_repo_manifest(
    repo_root: &Path,
) -> Result<(ContentRepoPaths, ContentRepoManifest)> {
    let paths = ContentRepoPaths::from_repo_root(repo_root);
    if !paths.repo_manifest_path.exists() {
        return Err(Error::manifest(format!(
            "content repo manifest not found at {}",
            paths.repo_manifest_path.display()
        )));
    }

    let repo_manifest_json = std::fs::read_to_string(&paths.repo_manifest_path)?;
    let repo_manifest: ContentRepoManifest = serde_json::from_str(&repo_manifest_json)?;

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
            format_version: "1.0.0".to_string(),
            updated_at: String::new(),
            entries: BTreeMap::new(),
        });
    }
    let json = std::fs::read_to_string(&paths.chunk_index_path)?;
    let index: manifest::ChunkIndex = serde_json::from_str(&json)?;
    Ok(index)
}

pub fn required_chunk_hashes(target_manifest: &TargetManifest) -> HashSet<String> {
    target_manifest
        .files
        .iter()
        .flat_map(|file| file.chunks.iter().map(|chunk| chunk.chunk_hash.clone()))
        .collect()
}

pub fn required_bundle_ids(
    target_manifest: &TargetManifest,
    chunk_index: &manifest::ChunkIndex,
) -> HashSet<String> {
    required_chunk_hashes(target_manifest)
        .into_iter()
        .filter_map(|chunk_hash| {
            chunk_index
                .entries
                .get(&chunk_hash)
                .map(|entry| entry.bundle_id.clone())
        })
        .collect()
}

pub fn scan_existing_store_state(paths: &ContentRepoPaths) -> Result<ExistingStoreState> {
    let mut state = ExistingStoreState::default();
    let chunk_index = load_chunk_index(paths)?;

    for entry in walkdir::WalkDir::new(&paths.bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() || !entry.path().extension().is_some_and(|ext| ext == "bundle") {
            continue;
        }

        let Some(bundle_id) = entry.path().file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let bundle_id = bundle_id.to_string();
        state.bundle_ids.insert(bundle_id.clone());
    }

    if chunk_index.entries.is_empty() {
        let rebuilt_chunk_index = rebuild_chunk_index_from_bundles(paths)?;
        state.chunk_locations =
            existing_chunk_locations_from_index(&rebuilt_chunk_index, &state.bundle_ids);
    } else {
        state.chunk_locations = existing_chunk_locations_from_index(&chunk_index, &state.bundle_ids);
    }

    for entry in walkdir::WalkDir::new(&paths.targets_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() || entry.file_name() != "manifest.json" {
            continue;
        }

        let manifest_json = std::fs::read_to_string(entry.path())?;
        let manifest: TargetManifest = serde_json::from_str(&manifest_json)?;
        for file in manifest.files {
            state
                .previous_file_manifests
                .entry(file.path.clone())
                .or_insert(file);
        }
    }

    Ok(state)
}

pub fn existing_chunk_locations_from_index(
    chunk_index: &manifest::ChunkIndex,
    existing_bundle_ids: &HashSet<String>,
) -> std::collections::HashMap<String, crate::ExistingChunkLocation> {
    chunk_index
        .entries
        .iter()
        .filter(|(_, entry)| existing_bundle_ids.contains(&entry.bundle_id))
        .map(|(chunk_hash, entry)| {
            (
                chunk_hash.clone(),
                crate::ExistingChunkLocation {
                    bundle_id: entry.bundle_id.clone(),
                    raw_size: entry.raw_size,
                    compressed_size: entry.compressed_size,
                },
            )
        })
        .collect()
}

pub fn rebuild_chunk_index_from_bundles(paths: &ContentRepoPaths) -> Result<manifest::ChunkIndex> {
    let mut entries = BTreeMap::new();
    for entry in walkdir::WalkDir::new(&paths.bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() || !entry.path().extension().is_some_and(|ext| ext == "bundle") {
            continue;
        }

        let Some(bundle_id) = entry.path().file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let bundle_id = bundle_id.to_string();
        let file = std::fs::File::open(entry.path())?;
        let reader = crate::bundle::BundleReader::open(file)?;
        for chunk in reader.entries() {
            entries.insert(
                chunk.chunk_hash.clone(),
                manifest::ChunkIndexEntry {
                    bundle_id: bundle_id.clone(),
                    raw_size: chunk.raw_size,
                    compressed_size: chunk.compressed_size,
                },
            );
        }
    }

    Ok(manifest::ChunkIndex {
        format_version: manifest::FORMAT_VERSION.to_string(),
        updated_at: chrono::DateTime::<chrono::Utc>::from(SystemTime::now()).to_rfc3339(),
        entries,
    })
}

pub fn write_chunk_index(paths: &ContentRepoPaths, index: &manifest::ChunkIndex) -> Result<()> {
    let json = serde_json::to_string_pretty(index)?;
    std::fs::write(&paths.chunk_index_path, json)?;
    Ok(())
}

pub fn write_content_repo_manifest(
    paths: &ContentRepoPaths,
    repo_manifest: &ContentRepoManifest,
) -> Result<()> {
    let json = serde_json::to_string_pretty(repo_manifest)?;
    std::fs::write(&paths.repo_manifest_path, json)?;
    Ok(())
}

pub fn compute_target_id_from_files(files: &[FileManifest]) -> String {
    let mut sorted_files = files.to_vec();
    sorted_files.sort_by(|left, right| left.path.cmp(&right.path));

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"gutch-target-content-id-v1");
    for file in &sorted_files {
        hasher.update(file.path.as_bytes());
        hasher.update(b"\x00");
        hasher.update(&file.size.to_le_bytes());
        hasher.update(b"\x00");
        hasher.update(&[u8::from(file.executable)]);
        hasher.update(b"\x00");
        hasher.update(file.full_file_hash.as_bytes());
        hasher.update(b"\x00");
    }
    hash_hex(&hasher.finalize())
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

    candidates.sort_by(|left, right| left.path.cmp(&right.path));
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
    let chunk_index_path = bundles_root
        .parent()
        .map(|parent| parent.join("chunk_index.json"))
        .unwrap_or_else(|| PathBuf::from("chunk_index.json"));
    let chunk_index = if chunk_index_path.exists() {
        let chunk_index_json = std::fs::read_to_string(&chunk_index_path)?;
        serde_json::from_str(&chunk_index_json)?
    } else {
        manifest::ChunkIndex {
            format_version: "1.0.0".to_string(),
            updated_at: String::new(),
            entries: BTreeMap::new(),
        }
    };
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
        let manifest: TargetManifest = serde_json::from_str(&manifest_json)?;
        let required_bundle_ids = required_bundle_ids(&manifest, &chunk_index);
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
    let resolved =
        resolve_target_identity(path, &manifest::TargetTagSet::default(), rules, &context).ok()?;

    let folder_name = path.file_name()?.to_str()?.to_string();

    Some(SourceTargetInstallCandidate {
        path: path.display().to_string(),
        folder_name,
        target_id: preview_target_identity(&resolved.target_tags),
        version_id: exclusive_tag_or_empty(&resolved.target_tags, "version"),
        platform: exclusive_tag_or_empty(&resolved.target_tags, "os"),
        architecture: exclusive_tag_or_empty(&resolved.target_tags, "arch"),
        tier_id: exclusive_tag_or_empty(&resolved.target_tags, "tier"),
        target_tags: resolved.target_tags,
    })
}

fn exclusive_tag_or_empty(target_tags: &manifest::TargetTagSet, prefix: &str) -> String {
    target_tags
        .get_exclusive(prefix)
        .unwrap_or_default()
        .to_string()
}

fn preview_target_identity(target_tags: &manifest::TargetTagSet) -> String {
    let mut parts = Vec::new();
    for prefix in ["version", "os", "arch", "tier"] {
        if let Some(value) = target_tags.get_exclusive(prefix) {
            parts.push(value.to_string());
        }
    }
    if parts.is_empty() {
        return target_tags.to_canonical_strings().join(" ");
    }
    parts.join(" ")
}

#[cfg(test)]
#[path = "repo_tests.rs"]
mod tests;
