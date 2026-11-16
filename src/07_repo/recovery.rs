//! Rebuilds the chunk index from stored artifacts.

use crate::Error;
use crate::Result;
use crate::artifact::bundle::BundleReader;
use crate::artifact::format::size_u32;
use crate::artifact::hash::ChunkHash;
use crate::artifact::loose::read_loose_chunk;
use crate::artifact::loose::read_loose_header;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::artifact::reference::ArtifactRef;
use crate::progress::OperationControl;
use crate::repo::index::ChunkIndex;
use crate::repo::index::ChunkIndexEntry;
use crate::repo::index::write_chunk_index;
use crate::repo::paths::load_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::path::Path;

use rayon::prelude::*;

use crate::repo::paths::RepoPaths;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoIndexRebuildResult {
    pub complete: bool,
    pub published: bool,
    pub chunk_count: usize,
    pub artifact_count: usize,
    pub duplicate_locations: usize,
    pub corrupt_artifacts: Vec<String>,
    pub missing_chunks: Vec<String>,
    pub affected_snapshots: Vec<String>,
}

/// Rebuild the chunk index from verified artifacts without replacing the current one.
pub fn plan_rebuild_repo_index(
    repo_root: &Path,
    control: &OperationControl,
) -> Result<RepoIndexRebuildResult> {
    recover_index(repo_root, false, control)
}

/// Rebuild the chunk index from verified artifacts and replace the current one
/// only when every retained snapshot resolves.
pub fn rebuild_repo_index(
    repo_root: &Path,
    control: &OperationControl,
) -> Result<RepoIndexRebuildResult> {
    recover_index(repo_root, true, control)
}

/// Discover and verify owned artifacts independently of the current chunk map.
fn recover_index(
    repo_root: &Path,
    publish: bool,
    control: &OperationControl,
) -> Result<RepoIndexRebuildResult> {
    let work = || {
        let (paths, _) = load_repo_manifest(repo_root)?;
        let mut entries: BTreeMap<ChunkHash, ChunkIndexEntry> = BTreeMap::new();
        let mut result = RepoIndexRebuildResult {
            complete: false,
            published: false,
            chunk_count: 0,
            artifact_count: 0,
            duplicate_locations: 0,
            corrupt_artifacts: Vec::new(),
            missing_chunks: Vec::new(),
            affected_snapshots: Vec::new(),
        };
        let artifacts = super::inventory::list_repo_artifact_paths(&paths)?;
        for (artifact, path) in artifacts {
            control.checkpoint()?;
            match recover_artifact_chunks(&artifact, &path) {
                Err(error) => result
                    .corrupt_artifacts
                    .push(format!("{}: {error}", artifact.id())),
                Ok(chunks) => {
                    result.artifact_count += 1;
                    for (hash, entry) in chunks {
                        match add_recovered_chunk(&mut entries, hash, entry) {
                            ChunkLocation::New => {}
                            ChunkLocation::Duplicate => result.duplicate_locations += 1,
                            ChunkLocation::ConflictingSize => {
                                result.duplicate_locations += 1;
                                result
                                    .corrupt_artifacts
                                    .push(format!("conflicting sizes for chunk {hash}"));
                            }
                        }
                    }
                }
            }
        }
        for snapshot in super::inventory::list_repo_snapshot_ids(&paths)? {
            control.checkpoint()?;
            let manifest = load_snapshot_manifest(&paths, &snapshot)?;
            let mut affected = false;
            for chunk in manifest.files().flat_map(|file| &file.chunks) {
                if !entries
                    .get(&chunk.chunk_hash)
                    .is_some_and(|entry| entry.raw_size == chunk.raw_size)
                {
                    affected = true;
                    result.missing_chunks.push(chunk.chunk_hash.to_hex());
                }
            }
            if affected {
                result.affected_snapshots.push(snapshot);
            }
        }
        result.missing_chunks.sort();
        result.missing_chunks.dedup();
        result.corrupt_artifacts.sort();
        result.affected_snapshots.sort();
        result.chunk_count = entries.len();
        result.complete = result.corrupt_artifacts.is_empty() && result.missing_chunks.is_empty();
        if publish && result.complete {
            control.checkpoint()?;
            write_chunk_index(
                &paths,
                &ChunkIndex {
                    entries: entries.into_iter().collect(),
                },
            )?;
            result.published = true;
        }
        Ok(result)
    };
    if publish {
        super::coordination::write_with_control(repo_root, control, work)
    } else {
        super::coordination::read_with_control(repo_root, control, work)
    }
}

/// Rebuilds the chunk index from every stored bundle and loose chunk with the
/// same identity checks and duplicate rules as `rebuild_repo_index`, failing on the
/// first unreadable artifact or conflicting chunk size.
pub(crate) fn rebuild_chunk_index_from_artifacts(
    paths: &RepoPaths,
    pool: &rayon::ThreadPool,
) -> Result<ChunkIndex> {
    let artifacts = super::inventory::list_repo_artifact_paths(paths)?;
    let recovered = pool.install(|| {
        artifacts
            .par_iter()
            .map(|(artifact, path)| recover_artifact_chunks(artifact, path))
            .collect::<Result<Vec<_>>>()
    })?;
    let mut entries = BTreeMap::new();
    for (hash, entry) in recovered.into_iter().flatten() {
        if let ChunkLocation::ConflictingSize = add_recovered_chunk(&mut entries, hash, entry) {
            return Err(Error::repo(format!("conflicting sizes for chunk {hash}")));
        }
    }
    Ok(ChunkIndex {
        entries: entries.into_iter().collect(),
    })
}

/// Reads and verifies one artifact, returning the index entry for each chunk
/// it holds.
fn recover_artifact_chunks(
    artifact: &ArtifactRef,
    path: &Path,
) -> Result<Vec<(ChunkHash, ChunkIndexEntry)>> {
    let size = size_u32(fs::metadata(path)?.len())?;
    match artifact {
        ArtifactRef::Bundle { id } => {
            let mut reader = BundleReader::open(File::open(path)?)?;
            reader.verify_identity(id)?;
            let shared_id = smol_str::SmolStr::new(id);
            Ok(reader
                .entries()
                .iter()
                .enumerate()
                .map(|(ordinal, chunk)| {
                    (
                        chunk.chunk_hash,
                        ChunkIndexEntry::new(
                            size,
                            ordinal as u32,
                            shared_id.clone(),
                            chunk.raw_size,
                            ArtifactStorageMode::Bundled,
                        ),
                    )
                })
                .collect())
        }
        ArtifactRef::LooseChunk { hash } => {
            let (declared, raw, _) = read_loose_header(&mut File::open(path)?)?;
            if declared != *hash {
                return Err(Error::repo("loose artifact filename disagrees with header"));
            }
            read_loose_chunk(path, *hash, u64::from(raw), None)?;
            Ok(vec![(
                *hash,
                ChunkIndexEntry::new(
                    size,
                    0,
                    hash.to_hex().into(),
                    raw,
                    ArtifactStorageMode::LooseChunks,
                ),
            )])
        }
    }
}

enum ChunkLocation {
    New,
    Duplicate,
    ConflictingSize,
}

/// Adds a recovered chunk location. When several artifacts hold the chunk,
/// the lowest artifact ID and ordinal wins regardless of enumeration order.
fn add_recovered_chunk(
    entries: &mut BTreeMap<ChunkHash, ChunkIndexEntry>,
    hash: ChunkHash,
    entry: ChunkIndexEntry,
) -> ChunkLocation {
    let Some(previous) = entries.get(&hash) else {
        entries.insert(hash, entry);
        return ChunkLocation::New;
    };
    let location = if previous.raw_size == entry.raw_size {
        ChunkLocation::Duplicate
    } else {
        ChunkLocation::ConflictingSize
    };
    if (entry.bundle_id.as_str(), entry.chunk_ordinal)
        < (previous.bundle_id.as_str(), previous.chunk_ordinal)
    {
        entries.insert(hash, entry);
    }
    location
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(id: &str, ordinal: u32, raw_size: u32) -> ChunkIndexEntry {
        ChunkIndexEntry::new(
            64,
            ordinal,
            id.into(),
            raw_size,
            ArtifactStorageMode::Bundled,
        )
    }

    #[test]
    fn duplicate_chunks_keep_the_lowest_location_in_any_order() {
        let hash = ChunkHash::of(b"chunk");
        let candidates = [
            location("b", 0, 5),
            location("a", 3, 5),
            location("a", 1, 5),
        ];
        for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0]] {
            let mut entries = BTreeMap::new();
            let locations = order
                .map(|index| add_recovered_chunk(&mut entries, hash, candidates[index].clone()));
            assert!(matches!(locations[0], ChunkLocation::New));
            assert!(matches!(locations[1], ChunkLocation::Duplicate));
            assert_eq!(entries[&hash], candidates[2]);
        }
        let mut entries = BTreeMap::from([(hash, location("a", 0, 5))]);
        assert!(matches!(
            add_recovered_chunk(&mut entries, hash, location("b", 0, 6)),
            ChunkLocation::ConflictingSize
        ));
    }
}
