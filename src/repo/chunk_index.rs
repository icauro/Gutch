use std::collections::{BTreeMap, HashSet};
use std::io::Cursor;
use std::time::SystemTime;

use crate::hash::ChunkHash;
use crate::manifest::{self, ArtifactStorageMode, FileManifest, binary, target::TargetManifest};
use crate::{Result, bundle_id_from_bundle_path, bundle_path, chunk_path, load_target_manifest};

use super::GutchRepoPaths;

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

fn empty_chunk_index() -> manifest::ChunkIndex {
    manifest::ChunkIndex {
        format_version: manifest::FORMAT_VERSION.to_string(),
        updated_at: String::new(),
        entries: BTreeMap::new(),
    }
}

pub fn load_chunk_index(paths: &GutchRepoPaths) -> Result<manifest::ChunkIndex> {
    if paths.chunk_index_path.exists() {
        return binary::load_chunk_index_binary(&paths.chunk_index_path);
    }
    Ok(empty_chunk_index())
}

pub fn required_chunk_hashes(target_manifest: &TargetManifest) -> HashSet<&str> {
    target_manifest
        .files
        .iter()
        .flat_map(|file| file.chunks.iter().map(|chunk| chunk.chunk_hash.as_str()))
        .collect()
}

pub fn required_chunk_hashes_typed(target_manifest: &TargetManifest) -> Result<HashSet<ChunkHash>> {
    target_manifest
        .files
        .iter()
        .flat_map(|file| file.chunks.iter())
        .map(|chunk| ChunkHash::parse_hex(&chunk.chunk_hash))
        .collect()
}

pub fn chunk_index_entries_typed(
    chunk_index: &manifest::ChunkIndex,
) -> Result<std::collections::HashMap<ChunkHash, &manifest::ChunkIndexEntry>> {
    chunk_index
        .entries
        .iter()
        .map(|(chunk_hash, entry)| Ok((ChunkHash::parse_hex(chunk_hash)?, entry)))
        .collect()
}

pub fn required_bundle_ids(
    target_manifest: &TargetManifest,
    chunk_index: &manifest::ChunkIndex,
) -> HashSet<String> {
    target_manifest
        .files
        .iter()
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

pub fn write_chunk_index(paths: &GutchRepoPaths, index: &manifest::ChunkIndex) -> Result<()> {
    binary::write_chunk_index_binary(&paths.chunk_index_path, index)
}
