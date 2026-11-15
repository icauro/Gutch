//! Rebuilds the chunk index from stored artifacts.

use crate::Error;
use crate::Result;
use crate::artifact::bundle::BundleReader;
use crate::artifact::format::FORMAT_VERSION;
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
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexRebuildResult {
    pub complete: bool,
    pub published: bool,
    pub chunk_count: usize,
    pub artifact_count: usize,
    pub duplicate_locations: usize,
    pub corrupt_artifacts: Vec<String>,
    pub missing_chunks: Vec<String>,
    pub affected_snapshots: Vec<String>,
}

/// Discover and verify owned artifacts independently of the current chunk map.
/// Publication is explicit and only allowed when every retained snapshot resolves.
pub fn rebuild_index(
    root: &Path,
    publish: bool,
    control: &OperationControl,
) -> Result<IndexRebuildResult> {
    let work = || {
        let (paths, _) = load_gutch_repo_manifest(root)?;
        let mut entries: BTreeMap<String, ChunkIndexEntry> = BTreeMap::new();
        let mut result = IndexRebuildResult {
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
                        add_recovered_chunk(&mut entries, &mut result, hash, entry);
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
                    .get(&chunk.chunk_hash.to_hex())
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
                    format_version: FORMAT_VERSION.into(),

                    entries: entries
                        .into_iter()
                        .map(|(hash, entry)| Ok((ChunkHash::parse_hex(&hash)?, entry)))
                        .collect::<Result<_>>()?,
                },
            )?;
            result.published = true;
        }
        Ok(result)
    };
    if publish {
        super::coordination::write_with_control(root, control, work)
    } else {
        super::coordination::read_with_control(root, control, work)
    }
}

/// Reads and verifies one artifact, returning the index entry for each chunk
/// it holds.
fn recover_artifact_chunks(
    artifact: &ArtifactRef,
    path: &Path,
) -> Result<Vec<(String, ChunkIndexEntry)>> {
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
                        chunk.chunk_hash.to_hex(),
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
            if declared.to_hex() != *hash {
                return Err(Error::repository(
                    "loose artifact filename disagrees with header",
                ));
            }
            read_loose_chunk(path, hash, u64::from(raw), None)?;
            Ok(vec![(
                hash.clone(),
                ChunkIndexEntry::new(
                    size,
                    0,
                    hash.as_str().into(),
                    raw,
                    ArtifactStorageMode::LooseChunks,
                ),
            )])
        }
    }
}

/// Adds a recovered chunk location. When several artifacts hold the chunk,
/// the lowest artifact ID and ordinal wins regardless of enumeration order.
fn add_recovered_chunk(
    entries: &mut BTreeMap<String, ChunkIndexEntry>,
    result: &mut IndexRebuildResult,
    hash: String,
    entry: ChunkIndexEntry,
) {
    if let Some(previous) = entries.get(&hash) {
        result.duplicate_locations += 1;
        if previous.raw_size != entry.raw_size {
            result
                .corrupt_artifacts
                .push(format!("conflicting sizes for chunk {hash}"));
        }
        if (previous.bundle_id.as_str(), previous.chunk_ordinal)
            <= (entry.bundle_id.as_str(), entry.chunk_ordinal)
        {
            return;
        }
    }
    entries.insert(hash, entry);
}
