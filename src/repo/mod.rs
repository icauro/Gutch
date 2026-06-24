use std::collections::{BTreeMap, HashSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

mod lookup;

use crate::hash::hash_hex;
use crate::manifest::CONTENT_REPO_MANIFEST_FILE;
use crate::manifest::{
    self, ArtifactStorageMode, FileManifest, binary, gutch_repo::GutchRepoManifest,
    target::TargetManifest,
};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct GutchRepoPaths {
    pub repo_root: PathBuf,
    pub repo_manifest_path: PathBuf,
    pub chunk_index_path: PathBuf,
    pub targets_root: PathBuf,
    pub bundles_root: PathBuf,
    pub chunks_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInspectStatus {
    pub exists: bool,
    pub is_repo: bool,
    pub repo_path: String,
    pub name: String,
    pub remote_repo_id: String,
    pub product_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_schema: Option<manifest::TargetTagSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_manifest: Option<manifest::gutch_repo::GutchRepoManifest>,
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

#[derive(Debug)]
pub struct ExistingStoreState {
    pub bundle_ids: HashSet<String>,
    pub chunk_index: manifest::ChunkIndex,
    pub chunk_locations: std::collections::HashMap<String, crate::ExistingChunkLocation>,
    pub previous_file_manifests: std::collections::HashMap<String, FileManifest>,
}

impl Default for ExistingStoreState {
    fn default() -> Self {
        Self {
            bundle_ids: HashSet::new(),
            chunk_index: empty_chunk_index(),
            chunk_locations: std::collections::HashMap::new(),
            previous_file_manifests: std::collections::HashMap::new(),
        }
    }
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

#[allow(dead_code)]
fn derive_repo_name_from_path(repo_root: &Path) -> String {
    repo_root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("base")
        .to_string()
}

pub fn ensure_gutch_repo_layout(
    paths: &GutchRepoPaths,
    name: &str,
) -> Result<GutchRepoManifest> {
    crate::helpers::create_dir_all(&paths.targets_root)?;
    crate::helpers::create_dir_all(&paths.bundles_root)?;
    crate::helpers::create_dir_all(&paths.chunks_root)?;

    let repo_manifest = if paths.repo_manifest_path.exists() {
        let repo_manifest_json = crate::helpers::read_to_string(&paths.repo_manifest_path)?;
        let repo_manifest: GutchRepoManifest =
            crate::helpers::from_json_str(&repo_manifest_json)?;
        repo_manifest
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

fn empty_chunk_index() -> manifest::ChunkIndex {
    manifest::ChunkIndex {
        format_version: manifest::FORMAT_VERSION.to_string(),
        updated_at: String::new(),
        entries: BTreeMap::new(),
    }
}

fn legacy_chunk_index_path(paths: &GutchRepoPaths) -> PathBuf {
    paths.repo_root.join("chunk_index.json")
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

pub fn load_chunk_index(paths: &GutchRepoPaths) -> Result<manifest::ChunkIndex> {
    if paths.chunk_index_path.exists() {
        return binary::load_chunk_index_binary(&paths.chunk_index_path);
    }

    let legacy_path = legacy_chunk_index_path(paths);
    if legacy_path.exists() {
        let json = crate::helpers::read_to_string(&legacy_path)?;
        let index: manifest::ChunkIndex = crate::helpers::from_json_str(&json)?;
        return Ok(index);
    }

    Ok(empty_chunk_index())
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
                .filter(|entry| entry.storage_mode == ArtifactStorageMode::Bundled)
                .map(|entry| entry.bundle_id.clone())
        })
        .collect()
}

pub fn required_loose_chunk_hashes(
    target_manifest: &TargetManifest,
    chunk_index: &manifest::ChunkIndex,
) -> HashSet<String> {
    required_chunk_hashes(target_manifest)
        .into_iter()
        .filter(|chunk_hash| {
            chunk_index
                .entries
                .get(chunk_hash)
                .is_some_and(|entry| entry.storage_mode == ArtifactStorageMode::LooseChunks)
        })
        .collect()
}

pub fn scan_existing_store_state(paths: &GutchRepoPaths) -> Result<ExistingStoreState> {
    let mut state = ExistingStoreState::default();
    let chunk_index = load_chunk_index(paths)?;

    for entry in walkdir::WalkDir::new(&paths.bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }

        let Some(bundle_id) = bundle_id_from_bundle_path(entry.path()) else {
            continue;
        };
        state.bundle_ids.insert(bundle_id);
    }

    state.chunk_index = if chunk_index.entries.is_empty() {
        rebuild_chunk_index_from_bundles(paths)?
    } else {
        chunk_index
    };
    state.chunk_locations = existing_chunk_locations_from_index(&state.chunk_index, paths);

    for entry in walkdir::WalkDir::new(&paths.targets_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }

        let Some(target_id) = entry.path().file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let manifest = load_target_manifest(paths, target_id)?;
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
    paths: &GutchRepoPaths,
) -> std::collections::HashMap<String, crate::ExistingChunkLocation> {
    chunk_index
        .entries
        .iter()
        .filter(|(chunk_hash, entry)| match entry.storage_mode {
            ArtifactStorageMode::Bundled => bundle_path(paths, &entry.bundle_id).is_file(),
            ArtifactStorageMode::LooseChunks => chunk_path(paths, chunk_hash).is_file(),
        })
        .map(|(chunk_hash, entry)| {
            (
                chunk_hash.clone(),
                crate::ExistingChunkLocation {
                    bundle_id: entry.bundle_id.clone(),
                    raw_size: entry.raw_size,
                    compressed_size: entry.compressed_size,
                    storage_mode: entry.storage_mode,
                },
            )
        })
        .collect()
}

pub fn rebuild_chunk_index_from_bundles(paths: &GutchRepoPaths) -> Result<manifest::ChunkIndex> {
    let mut entries = BTreeMap::new();
    for entry in walkdir::WalkDir::new(&paths.bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }

        let Some(bundle_id) = bundle_id_from_bundle_path(entry.path()) else {
            continue;
        };
        let file = std::fs::File::open(entry.path())?;
        let reader = crate::bundle::BundleReader::open(file)?;
        for chunk in reader.entries() {
            entries.insert(
                chunk.chunk_hash.clone(),
                manifest::ChunkIndexEntry {
                    bundle_id: bundle_id.clone(),
                    raw_size: chunk.raw_size,
                    compressed_size: chunk.compressed_size,
                    storage_mode: ArtifactStorageMode::Bundled,
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

pub fn add_built_bundles_to_chunk_index(
    chunk_index: &mut manifest::ChunkIndex,
    bundles: &[crate::BuiltBundle],
) -> Result<()> {
    for bundle in bundles {
        let reader = crate::bundle::BundleReader::open(Cursor::new(bundle.data.as_slice()))?;
        for chunk in reader.entries() {
            chunk_index.entries.insert(
                chunk.chunk_hash.clone(),
                manifest::ChunkIndexEntry {
                    bundle_id: bundle.bundle_id.clone(),
                    raw_size: chunk.raw_size,
                    compressed_size: chunk.compressed_size,
                    storage_mode: ArtifactStorageMode::Bundled,
                },
            );
        }
    }
    chunk_index.updated_at = chrono::DateTime::<chrono::Utc>::from(SystemTime::now()).to_rfc3339();
    Ok(())
}

pub fn add_built_loose_chunks_to_chunk_index(
    chunk_index: &mut manifest::ChunkIndex,
    chunks: &[crate::BuiltLooseChunk],
) -> Result<()> {
    for chunk in chunks {
        chunk_index.entries.insert(
            chunk.chunk_hash.clone(),
            manifest::ChunkIndexEntry {
                bundle_id: chunk.chunk_hash.clone(),
                raw_size: chunk.raw_size,
                compressed_size: chunk.compressed_size,
                storage_mode: ArtifactStorageMode::LooseChunks,
            },
        );
    }
    chunk_index.updated_at = chrono::DateTime::<chrono::Utc>::from(SystemTime::now()).to_rfc3339();
    Ok(())
}

pub fn write_chunk_index(paths: &GutchRepoPaths, index: &manifest::ChunkIndex) -> Result<()> {
    binary::write_chunk_index_binary(&paths.chunk_index_path, index)
}

pub fn write_gutch_repo_manifest(
    paths: &GutchRepoPaths,
    repo_manifest: &GutchRepoManifest,
) -> Result<()> {
    crate::helpers::write_json(&paths.repo_manifest_path, repo_manifest)
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

pub fn target_manifest_path(paths: &GutchRepoPaths, target_id: &str) -> PathBuf {
    paths.targets_root.join(format!("{target_id}.json"))
}

pub fn target_manifest_bin_path(paths: &GutchRepoPaths, target_id: &str) -> PathBuf {
    paths.targets_root.join(format!("{target_id}.bin"))
}

pub fn load_target_manifest(paths: &GutchRepoPaths, target_id: &str) -> Result<TargetManifest> {
    let manifest_path = target_manifest_path(paths, target_id);
    let manifest_json = crate::helpers::read_to_string(&manifest_path)?;
    load_target_manifest_from_json_text(paths, target_id, &manifest_json)
}

pub fn serialize_target_manifest_for_remote(target_manifest: &TargetManifest) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(target_manifest).map_err(Into::into)
}

pub fn write_target_manifest_artifacts(
    paths: &GutchRepoPaths,
    target_manifest: &TargetManifest,
) -> Result<(PathBuf, u64)> {
    crate::helpers::create_dir_all(&paths.targets_root)?;

    let manifest_path = target_manifest_path(paths, &target_manifest.target_id);
    let bin_path = target_manifest_bin_path(paths, &target_manifest.target_id);

    let shell = binary::target_manifest_shell_from_manifest(target_manifest);
    let manifest_json = serde_json::to_vec_pretty(&shell)?;
    crate::helpers::write(&manifest_path, &manifest_json)?;
    binary::write_target_manifest_files_binary(&bin_path, target_manifest)?;

    let manifest_size = std::fs::metadata(&manifest_path)?.len();
    Ok((manifest_path, manifest_size))
}

fn load_target_manifest_from_json_text(
    paths: &GutchRepoPaths,
    target_id: &str,
    manifest_json: &str,
) -> Result<TargetManifest> {
    let value: serde_json::Value = serde_json::from_str(manifest_json)?;
    if value.get("files").is_some() {
        let mut manifest: TargetManifest = serde_json::from_value(value)?;
        if manifest.target_id.trim().is_empty() {
            manifest.target_id = target_id.to_string();
        }
        return Ok(manifest);
    }

    let shell: binary::TargetManifestShell = serde_json::from_value(value)?;
    let bin_path = target_manifest_bin_path(paths, target_id);
    let files = binary::load_target_manifest_files_binary(&bin_path)?;
    Ok(binary::target_manifest_from_shell(
        shell,
        target_id.to_string(),
        files,
    ))
}

pub use lookup::{LookupTargetChangePlan, plan_lookup_target_changes};

pub fn inspect_gutch_repo(repo_root: &Path) -> Result<RepoInspectStatus> {
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

    let (repo_paths, repo_manifest) = load_gutch_repo_manifest(&repo_path)?;
    let (_bundle_ids, bundle_count, bundle_bytes) =
        inspect_bundle_inventory(&repo_paths.bundles_root)?;
    let targets = inspect_target_manifests(
        &repo_paths,
        &repo_paths.targets_root,
        &repo_paths.bundles_root,
        repo_manifest.target_schema.as_ref(),
    )?;

    Ok(RepoInspectStatus {
        exists: true,
        is_repo: true,
        repo_path: repo_path_display,
        name: repo_manifest.name.clone(),
        remote_repo_id: repo_manifest.remote_repo_id.clone(),
        product_id: repo_manifest.product_id.clone(),
        target_schema: repo_manifest.target_schema.clone(),
        repo_manifest: Some(repo_manifest),
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

    let (_repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
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
        name: String::new(),
        remote_repo_id: String::new(),
        product_id: String::new(),
        target_schema: None,
        repo_manifest: None,
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

        if let Some(bundle_id) = bundle_id_from_bundle_path(entry.path()) {
            bundle_count += 1;
            bundle_bytes += entry.metadata()?.len();
            bundle_ids.insert(bundle_id);
        }
    }

    Ok((bundle_ids, bundle_count, bundle_bytes))
}

fn inspect_target_manifests(
    repo_paths: &GutchRepoPaths,
    targets_root: &Path,
    bundles_root: &Path,
    _target_schema: Option<&manifest::TargetTagSchema>,
) -> Result<Vec<RepoInspectTargetStatus>> {
    if !targets_root.exists() {
        return Ok(Vec::new());
    }

    let (available_bundle_ids, _, _) = inspect_bundle_inventory(bundles_root)?;
    let chunk_index = load_chunk_index(repo_paths)?;
    let mut targets = Vec::new();
    for entry in std::fs::read_dir(targets_root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }

        let Some(target_id) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let manifest_path = path.clone();
        let manifest = load_target_manifest(repo_paths, target_id)?;
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
    use crate::core::target_identity::resolve_target_identity;

    let context = format!("source target '{}'", path.display());
    let resolved =
        resolve_target_identity(path, &manifest::TargetTagSet::default(), rules, &context).ok()?;

    let folder_name = path.file_name()?.to_str()?.to_string();

    Some(SourceTargetInstallCandidate {
        path: path.display().to_string(),
        folder_name,
        target_id: preview_target_identity(&resolved.target_tags, Some(rules)),
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

fn preview_target_identity(
    target_tags: &manifest::TargetTagSet,
    target_schema: Option<&manifest::TargetTagSchema>,
) -> String {
    target_tags.to_ordered_strings(target_schema).join(" ")
}

#[cfg(test)]
#[path = "repo_tests.rs"]
mod tests;
