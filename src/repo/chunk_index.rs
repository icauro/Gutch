use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rayon::prelude::*;

use crate::hash::ChunkHash;
use crate::manifest::{
    self, ArtifactStorageMode, FileManifest, binary, snapshot::SnapshotManifest,
};
use crate::{Result, bundle_id_from_bundle_path, chunk_path, load_snapshot_manifest};

use super::GutchRepoPaths;

#[derive(Debug)]
pub struct ExistingStoreState {
    pub bundle_ids: HashSet<String>,
    pub chunk_index: manifest::ChunkIndex,
    pub chunk_locations: std::collections::HashMap<String, crate::ExistingChunkLocation>,
    pub previous_file_manifests: std::collections::HashMap<String, Vec<FileManifest>>,
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

fn empty_chunk_index() -> manifest::ChunkIndex {
    manifest::ChunkIndex {
        format_version: manifest::FORMAT_VERSION.to_string(),
        updated_at: String::new(),
        entries: BTreeMap::new(),
    }
}

pub(crate) fn load_chunk_index(paths: &GutchRepoPaths) -> Result<manifest::ChunkIndex> {
    if paths.chunk_map_path.exists() {
        return binary::load_chunk_index_binary(&paths.chunk_map_path);
    }
    Ok(empty_chunk_index())
}

pub fn required_chunk_hashes(snapshot_manifest: &SnapshotManifest) -> HashSet<&str> {
    snapshot_manifest
        .files()
        .flat_map(|file| file.chunks.iter().map(|chunk| chunk.chunk_hash.as_str()))
        .collect()
}

pub fn required_chunk_hashes_typed(
    snapshot_manifest: &SnapshotManifest,
) -> Result<HashSet<ChunkHash>> {
    snapshot_manifest
        .files()
        .flat_map(|file| file.chunks.iter())
        .map(|chunk| ChunkHash::parse_hex(&chunk.chunk_hash))
        .collect()
}

pub fn required_bundle_ids(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: &manifest::ChunkIndex,
) -> HashSet<String> {
    snapshot_manifest
        .files()
        .flat_map(|file| file.chunks.iter())
        .filter_map(|chunk_ref| {
            chunk_index
                .entries
                .get(&chunk_ref.chunk_hash)
                .filter(|entry| entry.storage_mode == ArtifactStorageMode::Bundled)
                .map(|entry| entry.bundle_id.clone())
        })
        .collect()
}

/// Scan the existing store using the caller's operation pool.
///
/// The scan is read-only. Results are merged after the parallel work in the
/// same sorted order as the sequential implementation so duplicate index
/// entries and previous-manifest candidates remain deterministic.
pub fn scan_existing_store_state_with_pool(
    paths: &GutchRepoPaths,
    pool: &rayon::ThreadPool,
    control: &crate::GutchOperationControl,
) -> Result<ExistingStoreState> {
    let mut state = ExistingStoreState::default();
    let chunk_index = load_chunk_index(paths)?;
    let (_, repo_manifest) = super::load_gutch_repo_manifest(&paths.repo_root)?;
    let compression_profile = repo_manifest.compression_profile();

    let _bundle_ids_span =
        control.profile_span("capture.prepare_source.load_existing_store_state.bundle_ids");
    let bundle_paths = collect_file_paths(&paths.bundles_root)?;
    state.bundle_ids = pool.install(|| {
        bundle_paths
            .par_iter()
            .filter_map(|path| bundle_id_from_bundle_path(path))
            .collect()
    });
    drop(_bundle_ids_span);

    state.chunk_index = if chunk_index.entries.is_empty() {
        let _rebuild_span = control
            .profile_span("capture.prepare_source.load_existing_store_state.rebuild_chunk_index");
        let result = rebuild_chunk_index_from_bundles_with_pool(paths, &compression_profile, pool);
        drop(_rebuild_span);
        result?
    } else {
        chunk_index
    };
    let _locations_span = control
        .profile_span("capture.prepare_source.load_existing_store_state.check_chunk_locations");
    state.chunk_locations = existing_chunk_locations_from_index_with_pool_and_bundle_ids(
        &state.chunk_index,
        paths,
        pool,
        &state.bundle_ids,
    )?;
    drop(_locations_span);

    let _manifests_span = control
        .profile_span("capture.prepare_source.load_existing_store_state.load_snapshot_manifests");
    let snapshot_manifest_paths = collect_snapshot_manifest_paths(paths)?;
    let manifests = pool.install(|| {
        snapshot_manifest_paths
            .par_iter()
            .map(|path| {
                let Some(snapshot_id) = snapshot_id_from_manifest_path(path) else {
                    return Ok(None);
                };
                load_snapshot_manifest(paths, snapshot_id).map(Some)
            })
            .collect::<Result<Vec<_>>>()
    })?;
    for manifest in manifests.into_iter().flatten() {
        add_previous_file_manifests(&mut state.previous_file_manifests, manifest);
    }
    drop(_manifests_span);

    Ok(state)
}

fn collect_file_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry?;
        if entry.path().is_file() {
            paths.push(entry.into_path());
        }
    }
    Ok(paths)
}

fn collect_snapshot_manifest_paths(paths: &GutchRepoPaths) -> Result<Vec<PathBuf>> {
    super::inventory::list_repo_snapshot_ids(paths)?
        .into_iter()
        .map(|snapshot_id| crate::snapshot_metadata_path(paths, &snapshot_id))
        .collect()
}

fn snapshot_id_from_manifest_path(path: &Path) -> Option<&str> {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
}

fn add_previous_file_manifests(
    previous_file_manifests: &mut std::collections::HashMap<String, Vec<FileManifest>>,
    manifest: SnapshotManifest,
) {
    for file in manifest.files() {
        let candidates = previous_file_manifests
            .entry(file.path.clone())
            .or_default();
        if !candidates.iter().any(|candidate| {
            candidate.size == file.size && candidate.full_file_hash == file.full_file_hash
        }) {
            candidates.push(file.clone());
        }
    }
}

pub(crate) fn existing_chunk_locations_from_index_with_pool_and_bundle_ids(
    chunk_index: &manifest::ChunkIndex,
    paths: &GutchRepoPaths,
    pool: &rayon::ThreadPool,
    available_bundle_ids: &HashSet<String>,
) -> Result<std::collections::HashMap<String, crate::ExistingChunkLocation>> {
    let locations = pool.install(|| {
        chunk_index
            .entries
            .par_iter()
            .map(|(chunk_hash, entry)| {
                let path_exists = match entry.storage_mode {
                    ArtifactStorageMode::Bundled => available_bundle_ids.contains(&entry.bundle_id),
                    ArtifactStorageMode::LooseChunks => chunk_path(paths, chunk_hash)?.is_file(),
                };
                Ok(path_exists.then(|| {
                    (
                        chunk_hash.clone(),
                        crate::ExistingChunkLocation {
                            bundle_id: entry.bundle_id.clone(),
                            compressed_size: entry.compressed_size,
                            storage_mode: entry.storage_mode,
                        },
                    )
                }))
            })
            .collect::<Result<Vec<_>>>()
    })?;

    Ok(locations.into_iter().flatten().collect())
}

pub fn rebuild_chunk_index_from_bundles_with_pool(
    paths: &GutchRepoPaths,
    compression_profile: &crate::manifest::CompressionProfile,
    pool: &rayon::ThreadPool,
) -> Result<manifest::ChunkIndex> {
    let bundle_paths = collect_file_paths(&paths.bundles_root)?;
    let bundle_entries = pool.install(|| {
        bundle_paths
            .par_iter()
            .map(|path| read_bundle_index_entries(path, compression_profile))
            .collect::<Result<Vec<_>>>()
    })?;
    build_chunk_index_from_bundle_entries(bundle_entries)
}

fn read_bundle_index_entries(
    path: &Path,
    compression_profile: &crate::manifest::CompressionProfile,
) -> Result<Vec<(String, manifest::ChunkIndexEntry)>> {
    let Some(bundle_id) = bundle_id_from_bundle_path(path) else {
        return Ok(Vec::new());
    };
    let file = std::fs::File::open(path)?;
    let reader = crate::bundle::BundleReader::open(file, compression_profile)?;
    Ok(reader
        .entries()
        .iter()
        .map(|chunk| {
            (
                chunk.chunk_hash.clone(),
                manifest::ChunkIndexEntry {
                    bundle_id: bundle_id.clone(),
                    raw_size: chunk.raw_size,
                    compressed_size: chunk.compressed_size,
                    storage_mode: ArtifactStorageMode::Bundled,
                },
            )
        })
        .collect())
}

fn build_chunk_index_from_bundle_entries(
    bundle_entries: Vec<Vec<(String, manifest::ChunkIndexEntry)>>,
) -> Result<manifest::ChunkIndex> {
    let mut entries = BTreeMap::new();
    for bundle_entries in bundle_entries {
        for (chunk_hash, entry) in bundle_entries {
            entries.insert(chunk_hash, entry);
        }
    }

    Ok(manifest::ChunkIndex {
        format_version: manifest::FORMAT_VERSION.to_string(),
        updated_at: chrono::DateTime::<chrono::Utc>::from(SystemTime::now()).to_rfc3339(),
        entries,
    })
}

pub(crate) fn add_built_bundles_to_chunk_index(
    chunk_index: &mut manifest::ChunkIndex,
    bundles: &[crate::BuiltBundle],
    _compression_profile: &crate::manifest::CompressionProfile,
) -> Result<()> {
    for bundle in bundles {
        for chunk in &bundle.entries {
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

pub(crate) fn add_built_loose_chunks_to_chunk_index(
    chunk_index: &mut manifest::ChunkIndex,
    chunks: &[crate::BuiltLooseChunk],
) -> Result<()> {
    for chunk in chunks {
        let chunk_hash = chunk.chunk_hash.to_string();
        chunk_index.entries.insert(
            chunk_hash.clone(),
            manifest::ChunkIndexEntry {
                bundle_id: chunk_hash,
                raw_size: chunk.raw_size,
                compressed_size: chunk.compressed_size,
                storage_mode: ArtifactStorageMode::LooseChunks,
            },
        );
    }
    chunk_index.updated_at = chrono::DateTime::<chrono::Utc>::from(SystemTime::now()).to_rfc3339();
    Ok(())
}

pub(crate) fn write_chunk_index(
    paths: &GutchRepoPaths,
    index: &manifest::ChunkIndex,
) -> Result<()> {
    binary::write_chunk_index_binary(&paths.chunk_map_path, index)
}
