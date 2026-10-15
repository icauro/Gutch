use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use crate::Error;
use crate::Result;
use crate::artifact::hash::ChunkHash;

use crate::artifact::format::CHUNK_INDEX_MAGIC as CHUNK_INDEX_BINARY_MAGIC;

use crate::artifact::binary_io::{
    BinaryReader, read_chunk_hash, read_u32, write_chunk_hash, write_u32,
};

const LOOSE_CHUNK_ARTIFACT_ROW: u32 = u32::MAX;

pub(crate) fn load_chunk_index_binary(path: &Path) -> Result<crate::repo::index::ChunkIndex> {
    load_chunk_index_binary_from_bytes(&crate::artifact::metadata::read_file(path)?)
}

fn decode_artifact_table(
    bytes: &[u8],
) -> Result<Vec<std::sync::Arc<super::records::IndexedArtifact>>> {
    let mut reader = BinaryReader {
        inner: bytes,
        remaining: bytes.len() as u64,
    };
    let artifact_count = read_u32(&mut reader)?;
    reader.require_records(u64::from(artifact_count), 36)?;
    let artifact_ids: Vec<_> = (0..artifact_count)
        .map(|_| read_chunk_hash(&mut reader))
        .collect::<Result<_>>()?;
    let mut artifacts = Vec::with_capacity(artifact_count as usize);
    let mut previous_artifact_id = None;
    for artifact_id in artifact_ids {
        let artifact_size = read_u32(&mut reader)?;
        if artifact_size == 0 || previous_artifact_id.is_some_and(|value| artifact_id <= value) {
            return Err(Error::manifest("invalid or unsorted index artifact"));
        }
        previous_artifact_id = Some(artifact_id);
        artifacts.push(std::sync::Arc::new(super::records::IndexedArtifact {
            bundle_id: smol_str::SmolStr::new(artifact_id.to_hex()),
            artifact_size,
            storage_mode: crate::artifact::profiles::ArtifactStorageMode::Bundled,
        }));
    }
    if reader.remaining != 0 {
        return Err(Error::manifest("trailing artifact column bytes"));
    }
    Ok(artifacts)
}

fn read_chunk_index_binary(groups: &[Vec<u8>]) -> Result<crate::repo::index::ChunkIndex> {
    let artifacts = decode_artifact_table(&groups[0])?;
    let mut reader = BinaryReader {
        inner: groups[1].as_slice(),
        remaining: groups[1].len() as u64,
    };
    let chunk_count = read_u32(&mut reader)?;
    reader.require_records(u64::from(chunk_count), 44)?;
    let chunk_hashes: Vec<_> = (0..chunk_count)
        .map(|_| read_chunk_hash(&mut reader))
        .collect::<Result<_>>()?;
    let artifact_table_rows: Vec<_> = (0..chunk_count)
        .map(|_| read_u32(&mut reader))
        .collect::<Result<_>>()?;
    // Loose chunks have no artifact-table row: their identity is the chunk hash,
    // and this column carries encoded size instead of a bundle ordinal.
    let chunk_ordinals_or_artifact_sizes: Vec<_> = (0..chunk_count)
        .map(|_| read_u32(&mut reader))
        .collect::<Result<_>>()?;
    let chunk_raw_sizes: Vec<_> = (0..chunk_count)
        .map(|_| read_u32(&mut reader))
        .collect::<Result<_>>()?;
    let mut entries = crate::repo::index::ChunkIndexEntries::new();
    let mut bundle_raw_byte_totals = vec![0u64; artifacts.len()];
    let mut seen_bundle_chunk_ordinals = std::collections::HashSet::new();
    let mut previous_chunk_hash = None;
    for row in 0..chunk_count as usize {
        let chunk_hash = chunk_hashes[row];
        if previous_chunk_hash.is_some_and(|value| chunk_hash <= value) {
            return Err(Error::manifest(
                "chunk index keys must be unique and sorted",
            ));
        }
        previous_chunk_hash = Some(chunk_hash);
        let artifact_table_row = artifact_table_rows[row];
        let chunk_ordinal_or_artifact_size = chunk_ordinals_or_artifact_sizes[row];
        let raw_size = u64::from(chunk_raw_sizes[row]);
        if raw_size == 0 || raw_size > crate::artifact::format::MAX_RAW_CHUNK_SIZE {
            return Err(Error::manifest("invalid index chunk size"));
        }
        let (record, chunk_ordinal) = if artifact_table_row == LOOSE_CHUNK_ARTIFACT_ROW {
            if chunk_ordinal_or_artifact_size == 0 {
                return Err(Error::manifest("invalid loose artifact size"));
            }
            (
                std::sync::Arc::new(super::records::IndexedArtifact {
                    bundle_id: smol_str::SmolStr::new(chunk_hash.to_hex()),
                    artifact_size: chunk_ordinal_or_artifact_size,
                    storage_mode: crate::artifact::profiles::ArtifactStorageMode::LooseChunks,
                }),
                0,
            )
        } else {
            if !seen_bundle_chunk_ordinals
                .insert((artifact_table_row, chunk_ordinal_or_artifact_size))
            {
                return Err(Error::manifest("duplicate chunk ordinal"));
            }
            let record = artifacts
                .get(artifact_table_row as usize)
                .ok_or_else(|| Error::manifest("invalid artifact row"))?;
            bundle_raw_byte_totals[artifact_table_row as usize] = bundle_raw_byte_totals
                [artifact_table_row as usize]
                .checked_add(raw_size)
                .filter(|bundle_raw_bytes| {
                    *bundle_raw_bytes <= crate::artifact::format::MAX_RAW_BUNDLE_SIZE
                })
                .ok_or_else(|| Error::manifest("indexed bundle exceeds raw size limit"))?;
            (record.clone(), chunk_ordinal_or_artifact_size)
        };
        entries.insert(
            chunk_hash,
            crate::repo::index::ChunkIndexEntry {
                compressed_size: record.artifact_size,
                artifact: record,
                chunk_ordinal,
                raw_size: chunk_raw_sizes[row],
            },
        );
    }
    if reader.remaining != 0 {
        return Err(Error::manifest("chunk index contains trailing bytes"));
    }
    let raw_bytes_by_bundle_id: BTreeMap<_, _> = artifacts
        .iter()
        .zip(bundle_raw_byte_totals)
        .map(|(record, raw)| (record.bundle_id.as_str(), raw))
        .collect();
    // Encoded bytes are shared by a bundle. Attribute an approximate share to
    // each chunk for accounting; this value is not stored or used for integrity.
    for entry in entries.values_mut() {
        if entry.storage_mode == crate::artifact::profiles::ArtifactStorageMode::Bundled {
            let bundle_raw_bytes = raw_bytes_by_bundle_id[entry.bundle_id.as_str()];
            entry.compressed_size = crate::artifact::format::size_u32(
                u64::from(entry.artifact_size) * u64::from(entry.raw_size) / bundle_raw_bytes,
            )?;
        }
    }
    Ok(crate::repo::index::ChunkIndex {
        format_version: crate::artifact::format::FORMAT_VERSION.to_string(),
        entries,
    })
}

pub(crate) fn load_chunk_index_binary_from_bytes(
    bytes: &[u8],
) -> Result<crate::repo::index::ChunkIndex> {
    read_chunk_index_binary(&crate::artifact::metadata::decode(
        bytes,
        CHUNK_INDEX_BINARY_MAGIC,
        2,
    )?)
}

pub(crate) fn write_chunk_index_binary(
    path: &Path,
    index: &crate::repo::index::ChunkIndex,
) -> Result<()> {
    let mut artifacts = BTreeMap::new();
    for (hash, entry) in &index.entries {
        if entry.raw_size == 0
            || u64::from(entry.raw_size) > crate::artifact::format::MAX_RAW_CHUNK_SIZE
            || entry.artifact_size == 0
        {
            return Err(Error::manifest("invalid indexed artifact or chunk size"));
        }
        if entry.storage_mode == crate::artifact::profiles::ArtifactStorageMode::LooseChunks
            && (ChunkHash::parse_hex(&entry.bundle_id)? != *hash || entry.chunk_ordinal != 0)
        {
            return Err(Error::manifest(
                "loose index identity or ordinal is inconsistent",
            ));
        }
        if entry.storage_mode == crate::artifact::profiles::ArtifactStorageMode::Bundled {
            let id = entry.bundle_id.as_str();
            if let Some((_, size)) = artifacts.get(id) {
                if *size != entry.artifact_size {
                    return Err(Error::manifest("inconsistent indexed artifact sizes"));
                }
            } else {
                // Canonical hex sorts in the same order as its binary hash.
                // Validate and decode each distinct artifact identity once.
                artifacts.insert(id, (ChunkHash::parse_hex(id)?, entry.artifact_size));
            }
        }
    }
    let artifact_count =
        u32::try_from(artifacts.len()).map_err(|_| Error::manifest("too many index artifacts"))?;
    if artifact_count == LOOSE_CHUNK_ARTIFACT_ROW {
        return Err(Error::manifest("artifact index sentinel exhausted"));
    }
    let artifact_rows: BTreeMap<_, _> = artifacts
        .keys()
        .enumerate()
        .map(|(row, id)| (*id, row as u32))
        .collect();
    let mut artifact_group = Vec::new();
    write_u32(&mut artifact_group, artifact_count)?;
    for (id, _) in artifacts.values() {
        write_chunk_hash(&mut artifact_group, *id)?;
    }
    for (_, size) in artifacts.values() {
        write_u32(&mut artifact_group, *size)?;
    }
    let mut chunk_group = Vec::new();
    write_u32(
        &mut chunk_group,
        u32::try_from(index.entries.len()).map_err(|_| Error::manifest("too many index chunks"))?,
    )?;
    for hash in index.entries.keys() {
        write_chunk_hash(&mut chunk_group, *hash)?;
    }
    for entry in index.entries.values() {
        write_u32(
            &mut chunk_group,
            match entry.storage_mode {
                crate::artifact::profiles::ArtifactStorageMode::Bundled => {
                    artifact_rows[entry.bundle_id.as_str()]
                }
                crate::artifact::profiles::ArtifactStorageMode::LooseChunks => {
                    LOOSE_CHUNK_ARTIFACT_ROW
                }
            },
        )?;
    }
    for entry in index.entries.values() {
        write_u32(
            &mut chunk_group,
            match entry.storage_mode {
                crate::artifact::profiles::ArtifactStorageMode::Bundled => entry.chunk_ordinal,
                crate::artifact::profiles::ArtifactStorageMode::LooseChunks => entry.artifact_size,
            },
        )?;
    }
    for entry in index.entries.values() {
        write_u32(&mut chunk_group, entry.raw_size)?;
    }
    let bytes = crate::artifact::metadata::encode(
        CHUNK_INDEX_BINARY_MAGIC,
        &[artifact_group, chunk_group],
    )?;
    // Apply exactly the reader's invariants before publishing writer input.
    load_chunk_index_binary_from_bytes(&bytes)?;
    crate::filesystem::write_atomic_with(path, |file| {
        file.write_all(&bytes)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_storage_roundtrip_is_deterministic_and_shares_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("index");
        let bundle = smol_str::SmolStr::new("07".repeat(32));
        let loose = ChunkHash::from_bytes([3; 32]);
        let mut index = crate::repo::index::ChunkIndex {
            format_version: crate::artifact::format::FORMAT_VERSION.into(),
            entries: [
                (
                    ChunkHash::from_bytes([1; 32]),
                    crate::repo::index::ChunkIndexEntry::new(
                        100,
                        0,
                        bundle.clone(),
                        10,
                        20,
                        crate::artifact::profiles::ArtifactStorageMode::Bundled,
                    ),
                ),
                (
                    ChunkHash::from_bytes([2; 32]),
                    crate::repo::index::ChunkIndexEntry::new(
                        100,
                        1,
                        bundle,
                        20,
                        80,
                        crate::artifact::profiles::ArtifactStorageMode::Bundled,
                    ),
                ),
                (
                    loose,
                    crate::repo::index::ChunkIndexEntry::new(
                        60,
                        0,
                        loose.to_hex().into(),
                        4,
                        60,
                        crate::artifact::profiles::ArtifactStorageMode::LooseChunks,
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        };
        write_chunk_index_binary(&path, &index).unwrap();
        let first = std::fs::read(&path).unwrap();
        let groups =
            crate::artifact::metadata::decode(&first, CHUNK_INDEX_BINARY_MAGIC, 2).unwrap();
        for (group, offset, value) in [
            (0, 36, 0u32),
            (1, 100, u32::MAX - 1),
            (1, 116, 0),
            (1, 124, 0),
            (
                1,
                124,
                crate::artifact::format::MAX_RAW_CHUNK_SIZE as u32 + 1,
            ),
        ] {
            let mut invalid = groups.clone();
            invalid[group][offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            let bytes =
                crate::artifact::metadata::encode(CHUNK_INDEX_BINARY_MAGIC, &invalid).unwrap();
            assert!(
                load_chunk_index_binary_from_bytes(&bytes).is_err(),
                "group {group}, offset {offset}"
            );
        }
        let mut duplicate = groups.clone();
        let first_hash = duplicate[1][4..36].to_vec();
        duplicate[1][36..68].copy_from_slice(&first_hash);
        assert!(
            load_chunk_index_binary_from_bytes(
                &crate::artifact::metadata::encode(CHUNK_INDEX_BINARY_MAGIC, &duplicate).unwrap()
            )
            .is_err()
        );
        let restored = load_chunk_index_binary(&path).unwrap();
        assert_eq!(restored.entries.len(), 3);
        let entries: Vec<_> = restored.entries.values().collect();
        assert!(std::sync::Arc::ptr_eq(
            &entries[0].artifact,
            &entries[1].artifact
        ));
        assert_eq!(
            entries[2].storage_mode,
            crate::artifact::profiles::ArtifactStorageMode::LooseChunks
        );
        write_chunk_index_binary(&path, &restored).unwrap();
        assert_eq!(first, std::fs::read(&path).unwrap());
        for entry in index.entries.values_mut() {
            entry.compressed_size = 1;
        }
        write_chunk_index_binary(&path, &index).unwrap();
        assert_eq!(first, std::fs::read(&path).unwrap());
        index.entries.values_mut().last().unwrap().chunk_ordinal = 1;
        assert!(write_chunk_index_binary(&path, &index).is_err());
        assert_eq!(first, std::fs::read(&path).unwrap());
    }

    #[test]
    fn writer_rejects_conflicting_sizes_and_noncanonical_artifact_ids_before_publication() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("index");
        let index = crate::repo::index::ChunkIndex {
            format_version: crate::artifact::format::FORMAT_VERSION.into(),
            entries: (0..2u8)
                .map(|ordinal| {
                    (
                        ChunkHash::from_bytes([ordinal; 32]),
                        crate::repo::index::ChunkIndexEntry::new(
                            128,
                            u32::from(ordinal),
                            "ab".repeat(32).into(),
                            16,
                            64,
                            crate::artifact::profiles::ArtifactStorageMode::Bundled,
                        ),
                    )
                })
                .collect(),
        };
        write_chunk_index_binary(&path, &index).unwrap();
        let original = std::fs::read(&path).unwrap();
        let mut inconsistent = index.clone();
        let last = inconsistent.entries.values_mut().last().unwrap();
        std::sync::Arc::make_mut(&mut last.artifact).artifact_size += 1;
        assert!(write_chunk_index_binary(&path, &inconsistent).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);

        for invalid_id in ["AB".repeat(32), "xx".repeat(32), "ab".repeat(31)] {
            let mut invalid = index.clone();
            let last = invalid.entries.values_mut().last().unwrap();
            std::sync::Arc::make_mut(&mut last.artifact).bundle_id = invalid_id.into();
            assert!(write_chunk_index_binary(&path, &invalid).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
    }
}
