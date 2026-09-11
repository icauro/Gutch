use crate::Error;
use crate::Result;
use crate::manifest::ArtifactStorageMode;
use crate::manifest::ChunkIndex;
use crate::manifest::ChunkIndexEntry;
use crate::manifest::CompressionProfile;
use crate::progress::GutchOperationControl;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkMapRegenerationResult {
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
pub fn regenerate_chunk_map(
    root: &Path,
    publish: bool,
    control: &GutchOperationControl,
) -> Result<ChunkMapRegenerationResult> {
    let work = || {
        let (paths, _) = crate::repo::paths::load_gutch_repo_manifest(root)?;
        let profile = CompressionProfile::default();
        let mut entries: BTreeMap<String, ChunkIndexEntry> = BTreeMap::new();
        let mut result = ChunkMapRegenerationResult {
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
            let recovered = (|| -> Result<Vec<(String, ChunkIndexEntry)>> {
                let size = crate::artifact::format::size_u32(std::fs::metadata(&path)?.len())?;
                match &artifact {
                    crate::repo::artifacts::ArtifactRef::Bundle { id } => {
                        let mut reader = crate::artifact::bundle::BundleReader::open(
                            std::fs::File::open(&path)?,
                            &profile,
                        )?;
                        reader.verify_identity(id)?;
                        let shared_id = smol_str::SmolStr::new(id);
                        Ok(reader
                            .entries()
                            .iter()
                            .enumerate()
                            .map(|(ordinal, chunk)| {
                                (
                                    chunk.chunk_hash.clone(),
                                    ChunkIndexEntry {
                                        bundle_id: shared_id.clone(),
                                        raw_size: chunk.raw_size,
                                        compressed_size: chunk.compressed_size,
                                        storage_mode: ArtifactStorageMode::Bundled,
                                        artifact_size: size,
                                        chunk_ordinal: ordinal as u32,
                                    },
                                )
                            })
                            .collect())
                    }
                    crate::repo::artifacts::ArtifactRef::LooseChunk { hash } => {
                        let (declared, raw, _) = crate::artifact::loose::read_loose_header(
                            &mut std::fs::File::open(&path)?,
                        )?;
                        if declared.to_hex() != *hash {
                            return Err(Error::manifest(
                                "loose artifact filename disagrees with header",
                            ));
                        }
                        crate::artifact::loose::read_loose_chunk(
                            &path,
                            hash,
                            u64::from(raw),
                            &profile,
                        )?;
                        Ok(vec![(
                            hash.clone(),
                            ChunkIndexEntry {
                                bundle_id: hash.as_str().into(),
                                raw_size: raw,
                                compressed_size: size,
                                storage_mode: ArtifactStorageMode::LooseChunks,
                                artifact_size: size,
                                chunk_ordinal: 0,
                            },
                        )])
                    }
                }
            })();
            match recovered {
                Err(error) => result
                    .corrupt_artifacts
                    .push(format!("{}: {error}", artifact.id())),
                Ok(chunks) => {
                    result.artifact_count += 1;
                    for (hash, entry) in chunks {
                        if let Some(previous) = entries.get(&hash) {
                            result.duplicate_locations += 1;
                            if previous.raw_size != entry.raw_size {
                                result
                                    .corrupt_artifacts
                                    .push(format!("conflicting sizes for chunk {hash}"));
                            }
                            // Stable selection regardless of filesystem enumeration order.
                            if (previous.bundle_id.as_str(), previous.chunk_ordinal)
                                <= (entry.bundle_id.as_str(), entry.chunk_ordinal)
                            {
                                continue;
                            }
                        }
                        entries.insert(hash, entry);
                    }
                }
            }
        }
        for snapshot in super::inventory::list_repo_snapshot_ids(&paths)? {
            control.checkpoint()?;
            let manifest =
                crate::repo::snapshot_manifests::load_snapshot_manifest(&paths, &snapshot)?;
            let mut affected = false;
            for chunk in manifest.files().flat_map(|file| &file.chunks) {
                if !entries
                    .get(&chunk.chunk_hash)
                    .is_some_and(|entry| entry.raw_size == chunk.raw_size)
                {
                    affected = true;
                    result.missing_chunks.push(chunk.chunk_hash.clone());
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
            crate::repo::index::write_chunk_index(
                &paths,
                &ChunkIndex {
                    format_version: "1".into(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                    entries: entries
                        .into_iter()
                        .map(|(hash, entry)| {
                            Ok((crate::artifact::hash::ChunkHash::parse_hex(&hash)?, entry))
                        })
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
