//! Reuse state from the repository: known bundles and chunks.

use crate::Result;
use crate::artifact::format::size_u32;
use crate::artifact::hash::ChunkHash;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::capture::build::types::BuiltBundle;
use crate::capture::build::types::BuiltLooseChunk;
use crate::progress::OperationControl;
use crate::repo::index::ChunkIndex;
use crate::repo::index::ChunkIndexEntry;
use crate::repo::index::{collect_file_paths, empty_chunk_index, load_chunk_index};
use crate::repo::paths::{RepoPaths, bundle_id_from_bundle_path, chunk_path};
use crate::repo::recovery::rebuild_chunk_index_from_artifacts;
use rayon::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;

#[derive(Debug)]
pub struct CaptureReuseState {
    pub bundle_ids: HashSet<String>,
    pub chunk_index: ChunkIndex,
    /// Encoded byte estimates for chunks with usable local artifacts.
    /// Artifact locations remain authoritative in chunk_index.
    pub available_chunk_sizes: HashMap<ChunkHash, u64>,
}

impl Default for CaptureReuseState {
    fn default() -> Self {
        Self {
            bundle_ids: HashSet::new(),
            chunk_index: empty_chunk_index(),
            available_chunk_sizes: HashMap::new(),
        }
    }
}

/// Scan the existing store using the caller's operation pool.
///
/// The scan is read-only. Results are merged after the parallel work in the
/// same sorted order as the sequential implementation so duplicate index
/// entries and previous-manifest candidates remain deterministic.
pub fn load_capture_reuse_state(
    paths: &RepoPaths,
    pool: &rayon::ThreadPool,
    control: &OperationControl,
) -> Result<CaptureReuseState> {
    let mut state = CaptureReuseState::default();
    let chunk_index = load_chunk_index(paths)?;

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
        let result = rebuild_chunk_index_from_artifacts(paths, pool);
        drop(_rebuild_span);
        result?
    } else {
        chunk_index
    };
    let _locations_span = control.profile_span(
        "capture.prepare_source.load_existing_store_state.check_available_chunk_sizes",
    );
    state.available_chunk_sizes =
        available_chunk_sizes(&state.chunk_index, paths, pool, &state.bundle_ids)?;
    drop(_locations_span);

    Ok(state)
}

pub(crate) fn available_chunk_sizes(
    chunk_index: &ChunkIndex,
    paths: &RepoPaths,
    pool: &rayon::ThreadPool,
    available_bundle_ids: &HashSet<String>,
) -> Result<HashMap<ChunkHash, u64>> {
    // Attribute encoded bytes across every indexed chunk, not just the current source's chunks.
    let mut raw_bytes_by_bundle = HashMap::new();
    for (_, entry) in &chunk_index.entries {
        if entry.storage_mode == ArtifactStorageMode::Bundled {
            *raw_bytes_by_bundle
                .entry(entry.bundle_id.as_str())
                .or_insert(0u64) += u64::from(entry.raw_size);
        }
    }
    let available_sizes = pool.install(|| {
        chunk_index
            .entries
            .iter()
            .map(|(chunk_hash, entry)| {
                let path_exists = match entry.storage_mode {
                    ArtifactStorageMode::Bundled => {
                        available_bundle_ids.contains(entry.bundle_id.as_str())
                    }
                    ArtifactStorageMode::LooseChunks => chunk_path(paths, *chunk_hash).is_file(),
                };
                Ok(path_exists.then(|| {
                    let encoded_size_estimate = match entry.storage_mode {
                        ArtifactStorageMode::Bundled => {
                            u64::from(entry.artifact_size) * u64::from(entry.raw_size)
                                / raw_bytes_by_bundle[entry.bundle_id.as_str()]
                        }
                        ArtifactStorageMode::LooseChunks => u64::from(entry.artifact_size),
                    };
                    (*chunk_hash, encoded_size_estimate)
                }))
            })
            .collect::<Result<Vec<_>>>()
    })?;

    Ok(available_sizes.into_iter().flatten().collect())
}

pub(crate) fn add_built_bundles_to_chunk_index(
    chunk_index: &mut ChunkIndex,
    bundles: &[BuiltBundle],
) -> Result<()> {
    let mut updates = Vec::new();
    for bundle in bundles {
        let shared_bundle_id = smol_str::SmolStr::new(&bundle.bundle_id);
        for (ordinal, chunk) in bundle.entries.iter().enumerate() {
            updates.push((
                chunk.chunk_hash,
                ChunkIndexEntry::new(
                    size_u32(bundle.size)?,
                    ordinal as u32,
                    shared_bundle_id.clone(),
                    chunk.raw_size,
                    ArtifactStorageMode::Bundled,
                ),
            ));
        }
    }
    chunk_index.entries.extend(updates);
    Ok(())
}

pub(crate) fn add_built_loose_chunks_to_chunk_index(
    chunk_index: &mut ChunkIndex,
    chunks: &[BuiltLooseChunk],
) -> Result<()> {
    let mut updates = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        updates.push((
            chunk.chunk_hash,
            ChunkIndexEntry::new(
                size_u32(chunk.compressed_size)?,
                0,
                chunk.chunk_hash.to_hex().into(),
                size_u32(chunk.raw_size)?,
                ArtifactStorageMode::LooseChunks,
            ),
        ));
    }
    chunk_index.entries.extend(updates);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ArtifactStorageMode::{Bundled, LooseChunks};

    #[test]
    fn available_estimates_share_bundle_bytes_and_keep_loose_sizes() {
        let hash_for = |byte| ChunkHash::from_bytes([byte; 32]);
        let root = tempfile::tempdir().unwrap();
        let paths = RepoPaths::from_repo_root(root.path());
        let bytes = b"data";
        let loose_hash = ChunkHash::of(bytes);
        let encoded = crate::test_util::loose_bytes(bytes, &Default::default());
        let loose_size = encoded.len() as u32;
        let loose_id = loose_hash.to_hex().into();
        let loose_path = chunk_path(&paths, loose_hash);
        std::fs::create_dir_all(loose_path.parent().unwrap()).unwrap();
        std::fs::write(&loose_path, encoded).unwrap();
        let bundle_a = smol_str::SmolStr::new("a1".repeat(32));
        let bundle_b = smol_str::SmolStr::new("b2".repeat(32));
        let rows = [
            (hash_for(1), bundle_a.clone(), 101, 1, 10, Bundled),
            (loose_hash, loose_id, loose_size, 0, 4, LooseChunks),
            (hash_for(3), bundle_b.clone(), 73, 0, 3, Bundled),
            (hash_for(4), bundle_a.clone(), 101, 0, 20, Bundled),
            (hash_for(5), bundle_b.clone(), 73, 1, 5, Bundled),
        ];
        let entries = rows
            .into_iter()
            .map(|(hash, id, size, ordinal, raw, mode)| {
                (
                    hash,
                    crate::repo::index::ChunkIndexEntry::new(size, ordinal, id, raw, mode),
                )
            })
            .collect();
        let index = crate::repo::index::ChunkIndex { entries };
        let pool = crate::runtime::build_thread_pool(1, "capture-test").unwrap();
        let available = HashSet::from([bundle_a.to_string(), bundle_b.to_string()]);
        let estimates = available_chunk_sizes(&index, &paths, &pool, &available).unwrap();
        for (hash, size) in [
            (hash_for(1), 33),
            (loose_hash, u64::from(loose_size)),
            (hash_for(3), 27),
            (hash_for(4), 67),
            (hash_for(5), 45),
        ] {
            assert_eq!(estimates.get(&hash), Some(&size));
        }

        let partial = crate::repo::index::ChunkIndex {
            entries: index
                .entries
                .iter()
                .filter(|(hash, _)| **hash != hash_for(4))
                .map(|(hash, entry)| (*hash, entry.clone()))
                .collect(),
        };
        let partial_estimates = available_chunk_sizes(&partial, &paths, &pool, &available).unwrap();
        assert_eq!(partial_estimates[&hash_for(1)], 101);
        let only_loose = available_chunk_sizes(&index, &paths, &pool, &HashSet::new()).unwrap();
        assert_eq!(only_loose.len(), 1);
        assert_eq!(only_loose[&loose_hash], u64::from(loose_size));
        std::fs::remove_file(loose_path).unwrap();
        assert!(
            available_chunk_sizes(&index, &paths, &pool, &HashSet::new(),)
                .unwrap()
                .is_empty()
        );
    }
}
