//! Chunk-to-artifact locations, index persistence, and required-artifact queries.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use rayon::prelude::*;

use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::manifest::snapshot::SnapshotManifest;
use crate::repo::paths::bundle_id_from_bundle_path;

use crate::repo::paths::GutchRepoPaths;

pub(crate) fn empty_chunk_index() -> ChunkIndex {
    ChunkIndex {
        format_version: crate::artifact::format::FORMAT_VERSION.to_string(),

        entries: ChunkIndexEntries::new(),
    }
}

pub(crate) fn load_chunk_index(paths: &GutchRepoPaths) -> Result<ChunkIndex> {
    if paths.chunk_map_path.exists() {
        return binary_codec::load_chunk_index_binary(&paths.chunk_map_path);
    }
    Ok(empty_chunk_index())
}

/// Merge an incoming index without losing usable cached locations or local-only chunks.
/// Remote locations replace shared chunks only when the local artifact is absent.
pub(crate) fn preserve_local_chunk_locations(
    paths: &GutchRepoPaths,
    incoming: &mut ChunkIndex,
) -> Result<()> {
    for (hash, entry) in load_chunk_index(paths)?.entries {
        let artifact =
            crate::repo::artifacts::ArtifactRef::from_chunk_entry(&hash.to_hex(), &entry);
        let artifact_is_cached = artifact.local_path(paths)?.is_file();
        let preserve_local_entry = artifact_is_cached || incoming.entries.get_hash(&hash).is_none();
        if preserve_local_entry {
            incoming.entries.insert(hash, entry);
        }
    }
    Ok(())
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
    chunk_index: &ChunkIndex,
) -> Result<HashSet<String>> {
    Ok(
        crate::repo::artifacts::required_artifacts(snapshot_manifest, chunk_index)?
            .into_iter()
            .filter_map(|artifact| match artifact {
                crate::repo::artifacts::ArtifactRef::Bundle { id } => Some(id),
                crate::repo::artifacts::ArtifactRef::LooseChunk { .. } => None,
            })
            .collect(),
    )
}

pub(crate) fn collect_file_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry?;
        if entry.file_type().is_file() || (entry.file_type().is_symlink() && entry.path().is_file())
        {
            paths.push(entry.into_path());
        }
    }
    Ok(paths)
}

pub fn rebuild_chunk_index_from_bundles_with_pool(
    paths: &GutchRepoPaths,
    compression_profile: &crate::artifact::profiles::CompressionProfile,
    pool: &rayon::ThreadPool,
) -> Result<ChunkIndex> {
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
    compression_profile: &crate::artifact::profiles::CompressionProfile,
) -> Result<Vec<(String, ChunkIndexEntry)>> {
    let Some(bundle_id) = bundle_id_from_bundle_path(path) else {
        return Ok(Vec::new());
    };
    let shared_bundle_id = smol_str::SmolStr::new(&bundle_id);
    let artifact_size = crate::artifact::format::size_u32(std::fs::metadata(path)?.len())?;
    let file = std::fs::File::open(path)?;
    let reader = crate::artifact::bundle::BundleReader::open(file, compression_profile)?;
    Ok(reader
        .entries()
        .iter()
        .enumerate()
        .map(|(ordinal, chunk)| {
            (
                chunk.chunk_hash.to_hex(),
                ChunkIndexEntry::new(
                    artifact_size,
                    ordinal as u32,
                    shared_bundle_id.clone(),
                    chunk.raw_size,
                    chunk.compressed_size,
                    ArtifactStorageMode::Bundled,
                ),
            )
        })
        .collect())
}

fn build_chunk_index_from_bundle_entries(
    bundle_entries: Vec<Vec<(String, ChunkIndexEntry)>>,
) -> Result<ChunkIndex> {
    let mut entries = std::collections::BTreeMap::new();
    for bundle_entries in bundle_entries {
        for (chunk_hash, entry) in bundle_entries {
            entries.insert(ChunkHash::parse_hex(&chunk_hash)?, entry);
        }
    }

    Ok(ChunkIndex {
        format_version: crate::artifact::format::FORMAT_VERSION.to_string(),

        entries: entries.into_iter().collect(),
    })
}

pub(crate) fn write_chunk_index(paths: &GutchRepoPaths, index: &ChunkIndex) -> Result<()> {
    binary_codec::write_chunk_index_binary(&paths.chunk_map_path, index)
}

pub(crate) mod binary_codec;
mod entries;
mod records;
pub use entries::ChunkIndexEntries;
pub use records::{ChunkIndex, ChunkIndexEntry};

#[cfg(test)]
mod cache_merge_tests {
    use super::*;

    #[test]
    fn incoming_indexes_keep_cached_and_local_only_locations_for_both_storage_modes() {
        for storage_mode in [
            ArtifactStorageMode::Bundled,
            ArtifactStorageMode::LooseChunks,
        ] {
            for (cached, shared) in [(true, true), (false, true), (false, false)] {
                let directory = tempfile::tempdir().unwrap();
                let paths = GutchRepoPaths::from_repo_root(directory.path());
                crate::repo::paths::ensure_gutch_repo_layout(&paths, "merge").unwrap();
                let chunk_hash = ChunkHash::from_blake3(blake3::hash(b"shared chunk"));
                let (local_bundle_id, remote_bundle_id) = match storage_mode {
                    ArtifactStorageMode::Bundled => (
                        blake3::hash(b"local bundle").to_hex().to_string(),
                        blake3::hash(b"remote bundle").to_hex().to_string(),
                    ),
                    ArtifactStorageMode::LooseChunks => (chunk_hash.to_hex(), chunk_hash.to_hex()),
                };
                let local_entry =
                    ChunkIndexEntry::new(128, 0, local_bundle_id.into(), 16, 128, storage_mode);
                let remote_entry =
                    ChunkIndexEntry::new(256, 0, remote_bundle_id.into(), 16, 256, storage_mode);
                let mut local = empty_chunk_index();
                local.entries.insert(chunk_hash, local_entry.clone());
                write_chunk_index(&paths, &local).unwrap();
                if cached {
                    let artifact = crate::repo::artifacts::ArtifactRef::from_chunk_entry(
                        &chunk_hash.to_hex(),
                        &local_entry,
                    );
                    let path = artifact.local_path(&paths).unwrap();
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(path, [0; 128]).unwrap();
                }
                let mut incoming = empty_chunk_index();
                if shared {
                    incoming.entries.insert(chunk_hash, remote_entry.clone());
                }
                let remote_only_hash = ChunkHash::from_blake3(blake3::hash(b"remote only chunk"));
                incoming
                    .entries
                    .insert(remote_only_hash, remote_entry.clone());

                preserve_local_chunk_locations(&paths, &mut incoming).unwrap();

                let expected = if cached || !shared {
                    &local_entry
                } else {
                    &remote_entry
                };
                assert_eq!(incoming.entries.get_hash(&chunk_hash), Some(expected));
                assert_eq!(
                    incoming.entries.get_hash(&remote_only_hash),
                    Some(&remote_entry)
                );
                // Planning merges only in memory; the persisted local index remains usable.
                assert_eq!(
                    load_chunk_index(&paths)
                        .unwrap()
                        .entries
                        .get_hash(&chunk_hash),
                    Some(&local_entry)
                );
            }
        }
    }
}
