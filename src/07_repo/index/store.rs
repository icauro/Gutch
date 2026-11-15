//! Loading, writing and merging the stored chunk index.

use super::binary_codec;
use super::entries::ChunkIndexEntries;
use super::records::ChunkIndex;
use crate::Result;
use crate::artifact::reference::ArtifactRef;
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

pub(crate) fn write_chunk_index(paths: &RepoPaths, index: &ChunkIndex) -> Result<()> {
    binary_codec::write_chunk_index_binary(&paths.chunk_map_path, index)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
