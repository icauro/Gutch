//! Chunk-to-artifact locations, index persistence, and required-artifact queries.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::manifest::snapshot::SnapshotManifest;

use crate::repo::paths::RepoPaths;

pub(crate) fn empty_chunk_index() -> ChunkIndex {
    ChunkIndex {
        entries: ChunkIndexEntries::new(),
    }
}

pub(crate) fn load_chunk_index(paths: &RepoPaths) -> Result<ChunkIndex> {
    if paths.chunk_map_path.exists() {
        return binary_codec::load_chunk_index_binary(&paths.chunk_map_path);
    }
    Ok(empty_chunk_index())
}

/// Merge an incoming index without losing usable cached locations or local-only chunks.
/// Remote locations replace shared chunks only when the local artifact is absent.
pub(crate) fn preserve_local_chunk_locations(
    paths: &RepoPaths,
    incoming: &mut ChunkIndex,
) -> Result<()> {
    for (hash, entry) in load_chunk_index(paths)?.entries {
        let artifact = ArtifactRef::from_chunk_entry(&hash, &entry);
        let artifact_is_cached = artifact.local_path(paths)?.is_file();
        let preserve_local_entry = artifact_is_cached || incoming.entries.get(&hash).is_none();
        if preserve_local_entry {
            incoming.entries.insert(hash, entry);
        }
    }
    Ok(())
}

pub fn required_chunk_hashes(snapshot_manifest: &SnapshotManifest) -> HashSet<ChunkHash> {
    snapshot_manifest
        .files()
        .flat_map(|file| file.chunks.iter().map(|chunk| chunk.chunk_hash))
        .collect()
}

pub fn required_bundle_ids(
    snapshot_manifest: &SnapshotManifest,
    chunk_index: &ChunkIndex,
) -> Result<HashSet<String>> {
    Ok(required_artifacts(snapshot_manifest, chunk_index)?
        .into_iter()
        .filter_map(|artifact| match artifact {
            ArtifactRef::Bundle { id } => Some(id.to_string()),
            ArtifactRef::LooseChunk { .. } => None,
        })
        .collect())
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

pub(crate) fn write_chunk_index(paths: &RepoPaths, index: &ChunkIndex) -> Result<()> {
    binary_codec::write_chunk_index_binary(&paths.chunk_map_path, index)
}

pub(crate) mod binary_codec;
mod entries;
mod records;
use crate::artifact::reference::ArtifactRef;
use crate::repo::artifacts::required_artifacts;
pub use entries::ChunkIndexEntries;
pub use records::{ChunkIndex, ChunkIndexEntry};

#[cfg(test)]
mod cache_merge_tests {
    use super::*;
    use crate::artifact::profiles::ArtifactStorageMode;

    #[test]
    fn incoming_indexes_keep_cached_and_local_only_locations_for_both_storage_modes() {
        for storage_mode in [
            ArtifactStorageMode::Bundled,
            ArtifactStorageMode::LooseChunks,
        ] {
            for (cached, shared) in [(true, true), (false, true), (false, false)] {
                let directory = tempfile::tempdir().unwrap();
                let paths = RepoPaths::from_repo_root(directory.path());
                crate::repo::paths::ensure_gutch_repo_layout(&paths, "merge").unwrap();
                let chunk_hash = ChunkHash::of(b"shared chunk");
                let (local_bundle_id, remote_bundle_id) = match storage_mode {
                    ArtifactStorageMode::Bundled => (
                        blake3::hash(b"local bundle").to_hex().to_string(),
                        blake3::hash(b"remote bundle").to_hex().to_string(),
                    ),
                    ArtifactStorageMode::LooseChunks => (chunk_hash.to_hex(), chunk_hash.to_hex()),
                };
                let local_entry =
                    ChunkIndexEntry::new(128, 0, local_bundle_id.into(), 16, storage_mode);
                let remote_entry =
                    ChunkIndexEntry::new(256, 0, remote_bundle_id.into(), 16, storage_mode);
                let mut local = empty_chunk_index();
                local.entries.insert(chunk_hash, local_entry.clone());
                write_chunk_index(&paths, &local).unwrap();
                if cached {
                    let artifact = crate::artifact::reference::ArtifactRef::from_chunk_entry(
                        &chunk_hash,
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
                let remote_only_hash = ChunkHash::of(b"remote only chunk");
                incoming
                    .entries
                    .insert(remote_only_hash, remote_entry.clone());

                preserve_local_chunk_locations(&paths, &mut incoming).unwrap();

                let expected = if cached || !shared {
                    &local_entry
                } else {
                    &remote_entry
                };
                assert_eq!(incoming.entries.get(&chunk_hash), Some(expected));
                assert_eq!(incoming.entries.get(&remote_only_hash), Some(&remote_entry));
                // Planning merges only in memory; the persisted local index remains usable.
                assert_eq!(
                    load_chunk_index(&paths).unwrap().entries.get(&chunk_hash),
                    Some(&local_entry)
                );
            }
        }
    }
}
