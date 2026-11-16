//! Reads one file of a snapshot straight from the repository's stored chunks.

use crate::artifact::bundle::BundleReader;
use crate::artifact::hash::ChunkHash;
use crate::artifact::loose::read_loose_chunk;
use crate::artifact::reference::ArtifactRef;
use crate::manifest::FileManifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::repo::coordination::read_with_control;
use crate::repo::index::{ChunkIndex, load_chunk_index};
use crate::repo::paths::load_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::{Error, Result};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::PathBuf;

/// One file of a snapshot to read without installing the snapshot.
#[derive(Debug, Clone, Default)]
pub struct SnapshotFileReadRequest {
    pub repo_root: PathBuf,
    pub snapshot_id: String,
    /// The file's path inside the snapshot, with `/` separators, as in the
    /// snapshot manifest's entries.
    pub path: String,
    pub control: OperationControl,
}

/// Returns one file's content, read from the bundles or loose chunks that hold
/// it. Only those artifacts are decoded, and each chunk and the whole file are
/// checked against their hashes. The content is returned in memory.
pub fn read_snapshot_file(request: &SnapshotFileReadRequest) -> Result<Vec<u8>> {
    let control = &request.control;
    read_with_control(&request.repo_root, control, || {
        let (paths, _) = load_repo_manifest(&request.repo_root)?;
        let manifest = load_snapshot_manifest(&paths, &request.snapshot_id)?;
        let file = find_snapshot_file(&manifest, &request.path)?;
        let chunk_index = load_chunk_index(&paths)?;

        let mut chunks: HashMap<ChunkHash, Vec<u8>> = HashMap::new();
        for (artifact, hashes) in file_artifacts(file, &chunk_index)? {
            control.checkpoint()?;
            let path = artifact.local_path(&paths)?;
            if !path.is_file() {
                return Err(Error::repo(format!(
                    "'{}' needs {} '{}', which this repository does not have; pull the snapshot first",
                    request.path,
                    if artifact.is_bundle() {
                        "bundle"
                    } else {
                        "loose chunk"
                    },
                    artifact.id()
                )));
            }
            match artifact {
                ArtifactRef::Bundle { .. } => {
                    let mut reader = BundleReader::open(File::open(&path)?)?;
                    let entries = reader.entries().to_vec();
                    let required: Vec<bool> = entries
                        .iter()
                        .map(|entry| hashes.contains(&entry.chunk_hash))
                        .collect();
                    reader.stream_bundle_chunks(None, &required, |index, bytes| {
                        chunks.insert(entries[index].chunk_hash, bytes.to_vec());
                        Ok(())
                    })?;
                }
                ArtifactRef::LooseChunk { hash } => {
                    let raw_size = chunk_index
                        .entries
                        .get(&hash)
                        .map_or(0, |entry| entry.raw_size);
                    let (bytes, _) = read_loose_chunk(&path, hash, u64::from(raw_size), None)?;
                    chunks.insert(hash, bytes);
                }
            }
        }

        let mut content = Vec::with_capacity(usize::try_from(file.size).unwrap_or(0));
        for chunk in &file.chunks {
            let bytes = chunks.get(&chunk.chunk_hash).ok_or_else(|| {
                Error::artifact(format!(
                    "chunk '{}' was not in its bundle",
                    chunk.chunk_hash
                ))
            })?;
            // Bundle streaming leaves chunk hashes to the caller.
            if ChunkHash::from_blake3(blake3::hash(bytes)) != chunk.chunk_hash {
                return Err(Error::artifact(format!(
                    "chunk '{}' of '{}' is corrupt",
                    chunk.chunk_hash, request.path
                )));
            }
            content.extend_from_slice(bytes);
        }
        if content.len() as u64 != file.size
            || ChunkHash::from_blake3(blake3::hash(&content)) != file.full_file_hash
        {
            return Err(Error::artifact(format!(
                "'{}' does not match its snapshot hash",
                request.path
            )));
        }
        Ok(content)
    })
}

/// The file at `path` in a snapshot.
pub(crate) fn find_snapshot_file<'a>(
    manifest: &'a SnapshotManifest,
    path: &str,
) -> Result<&'a FileManifest> {
    manifest
        .files()
        .find(|file| file.path == path)
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "snapshot '{}' has no file '{path}'",
                manifest.snapshot_id
            ))
        })
}

/// The bundles and loose chunks that hold a file, each with the file's chunks in it.
pub(crate) fn file_artifacts(
    file: &FileManifest,
    chunk_index: &ChunkIndex,
) -> Result<HashMap<ArtifactRef, HashSet<ChunkHash>>> {
    let mut artifacts: HashMap<ArtifactRef, HashSet<ChunkHash>> = HashMap::new();
    for chunk in &file.chunks {
        let entry = chunk_index.entries.get(&chunk.chunk_hash).ok_or_else(|| {
            Error::repo(format!(
                "missing chunk index entry for chunk '{}'",
                chunk.chunk_hash
            ))
        })?;
        artifacts
            .entry(ArtifactRef::from_chunk_entry(&chunk.chunk_hash, entry))
            .or_default()
            .insert(chunk.chunk_hash);
    }
    Ok(artifacts)
}

#[cfg(test)]
#[path = "file_read_tests.rs"]
mod tests;
