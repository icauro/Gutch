use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use crate::Error;
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::manifest;

use crate::artifact::format::CHUNK_INDEX_MAGIC as CHUNK_INDEX_BINARY_MAGIC;

use crate::artifact::binary_io::*;
pub(crate) fn load_chunk_index_binary(path: &Path) -> Result<manifest::ChunkIndex> {
    load_chunk_index_binary_from_bytes(&crate::artifact::metadata::read_file(path)?)
}

fn read_chunk_index_binary(groups: &[Vec<u8>]) -> Result<manifest::ChunkIndex> {
    let mut reader = BinaryReader {
        inner: groups[0].as_slice(),
        remaining: groups[0].len() as u64,
    };
    let artifact_count = read_u32(&mut reader)?;
    reader.require_records(u64::from(artifact_count), 36)?;
    let ids: Vec<_> = (0..artifact_count)
        .map(|_| read_chunk_hash(&mut reader))
        .collect::<Result<_>>()?;
    let mut artifacts = Vec::with_capacity(artifact_count as usize);
    let mut previous = None;
    for id in ids {
        let size = read_u32(&mut reader)?;
        if size == 0 || previous.is_some_and(|value| id <= value) {
            return Err(Error::manifest("invalid or unsorted index artifact"));
        }
        previous = Some(id);
        artifacts.push(std::sync::Arc::new(super::records::IndexedArtifact {
            bundle_id: smol_str::SmolStr::new(id.to_hex()),
            artifact_size: size,
            storage_mode: manifest::ArtifactStorageMode::Bundled,
        }));
    }
    if reader.remaining != 0 {
        return Err(Error::manifest("trailing artifact column bytes"));
    }
    let mut reader = BinaryReader {
        inner: groups[1].as_slice(),
        remaining: groups[1].len() as u64,
    };
    let count = read_u32(&mut reader)?;
    reader.require_records(u64::from(count), 44)?;
    let hashes: Vec<_> = (0..count)
        .map(|_| read_chunk_hash(&mut reader))
        .collect::<Result<_>>()?;
    let artifact_rows: Vec<_> = (0..count)
        .map(|_| read_u32(&mut reader))
        .collect::<Result<_>>()?;
    let ordinals_or_sizes: Vec<_> = (0..count)
        .map(|_| read_u32(&mut reader))
        .collect::<Result<_>>()?;
    let raw_sizes: Vec<_> = (0..count)
        .map(|_| read_u32(&mut reader))
        .collect::<Result<_>>()?;
    let mut entries = manifest::ChunkIndexEntries::new();
    let mut raw_totals = vec![0u64; artifacts.len()];
    let mut ordinals = std::collections::HashSet::new();
    let mut previous = None;
    for row in 0..count as usize {
        let hash = hashes[row];
        if previous.is_some_and(|value| hash <= value) {
            return Err(Error::manifest(
                "chunk index keys must be unique and sorted",
            ));
        }
        previous = Some(hash);
        let artifact = artifact_rows[row];
        let ordinal_or_size = ordinals_or_sizes[row];
        let raw_size = u64::from(raw_sizes[row]);
        if raw_size == 0 || raw_size > crate::artifact::format::MAX_RAW_CHUNK_SIZE {
            return Err(Error::manifest("invalid index chunk size"));
        }
        let (record, chunk_ordinal) = if artifact == u32::MAX {
            if ordinal_or_size == 0 {
                return Err(Error::manifest("invalid loose artifact size"));
            }
            (
                std::sync::Arc::new(super::records::IndexedArtifact {
                    bundle_id: smol_str::SmolStr::new(hash.to_hex()),
                    artifact_size: ordinal_or_size,
                    storage_mode: manifest::ArtifactStorageMode::LooseChunks,
                }),
                0,
            )
        } else {
            if !ordinals.insert((artifact, ordinal_or_size)) {
                return Err(Error::manifest("duplicate chunk ordinal"));
            }
            let record = artifacts
                .get(artifact as usize)
                .ok_or_else(|| Error::manifest("invalid artifact row"))?;
            raw_totals[artifact as usize] = raw_totals[artifact as usize]
                .checked_add(raw_size)
                .filter(|total| *total <= crate::artifact::format::MAX_RAW_BUNDLE_SIZE)
                .ok_or_else(|| Error::manifest("indexed bundle exceeds raw size limit"))?;
            (record.clone(), ordinal_or_size)
        };
        entries.insert(
            hash,
            manifest::ChunkIndexEntry {
                compressed_size: record.artifact_size,
                artifact: record,
                chunk_ordinal,
                raw_size: raw_sizes[row],
            },
        );
    }
    if reader.remaining != 0 {
        return Err(Error::manifest("chunk index contains trailing bytes"));
    }
    let totals: BTreeMap<_, _> = artifacts
        .iter()
        .zip(raw_totals)
        .map(|(record, raw)| (record.bundle_id.as_str(), raw))
        .collect();
    for entry in entries.values_mut() {
        if entry.storage_mode == manifest::ArtifactStorageMode::Bundled {
            let total = totals[entry.bundle_id.as_str()];
            entry.compressed_size = crate::artifact::format::size_u32(
                u64::from(entry.artifact_size) * u64::from(entry.raw_size) / total,
            )?;
        }
    }
    Ok(manifest::ChunkIndex {
        format_version: manifest::FORMAT_VERSION.to_string(),
        entries,
    })
}

pub(crate) fn load_chunk_index_binary_from_bytes(bytes: &[u8]) -> Result<manifest::ChunkIndex> {
    read_chunk_index_binary(&crate::artifact::metadata::decode(
        bytes,
        CHUNK_INDEX_BINARY_MAGIC,
        2,
    )?)
}

pub(crate) fn write_chunk_index_binary(path: &Path, index: &manifest::ChunkIndex) -> Result<()> {
    let mut artifacts = BTreeMap::new();
    for (hash, entry) in &index.entries {
        if entry.raw_size == 0
            || u64::from(entry.raw_size) > crate::artifact::format::MAX_RAW_CHUNK_SIZE
            || entry.artifact_size == 0
        {
            return Err(Error::manifest("invalid indexed artifact or chunk size"));
        }
        if entry.storage_mode == manifest::ArtifactStorageMode::LooseChunks
            && (ChunkHash::parse_hex(&entry.bundle_id)? != *hash || entry.chunk_ordinal != 0)
        {
            return Err(Error::manifest(
                "loose index identity or ordinal is inconsistent",
            ));
        }
        if entry.storage_mode == manifest::ArtifactStorageMode::Bundled {
            let id = ChunkHash::parse_hex(&entry.bundle_id)?;
            if let Some(size) = artifacts.insert(id, entry.artifact_size) {
                if size != entry.artifact_size {
                    return Err(Error::manifest("inconsistent indexed artifact sizes"));
                }
            }
        }
    }
    let artifact_count =
        u32::try_from(artifacts.len()).map_err(|_| Error::manifest("too many index artifacts"))?;
    if artifact_count == u32::MAX {
        return Err(Error::manifest("artifact index sentinel exhausted"));
    }
    let artifact_rows: BTreeMap<_, _> = artifacts
        .keys()
        .enumerate()
        .map(|(row, id)| (*id, row as u32))
        .collect();
    let mut artifact_group = Vec::new();
    write_u32(&mut artifact_group, artifact_count)?;
    for id in artifacts.keys() {
        write_chunk_hash(&mut artifact_group, *id)?;
    }
    for size in artifacts.values() {
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
                manifest::ArtifactStorageMode::Bundled => {
                    artifact_rows[&ChunkHash::parse_hex(&entry.bundle_id)?]
                }
                manifest::ArtifactStorageMode::LooseChunks => u32::MAX,
            },
        )?;
    }
    for entry in index.entries.values() {
        write_u32(
            &mut chunk_group,
            match entry.storage_mode {
                manifest::ArtifactStorageMode::Bundled => entry.chunk_ordinal,
                manifest::ArtifactStorageMode::LooseChunks => entry.artifact_size,
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
        let mut index = manifest::ChunkIndex {
            format_version: manifest::FORMAT_VERSION.into(),
            entries: [
                (
                    ChunkHash::from_bytes([1; 32]),
                    manifest::ChunkIndexEntry::new(
                        100,
                        0,
                        bundle.clone(),
                        10,
                        20,
                        manifest::ArtifactStorageMode::Bundled,
                    ),
                ),
                (
                    ChunkHash::from_bytes([2; 32]),
                    manifest::ChunkIndexEntry::new(
                        100,
                        1,
                        bundle,
                        20,
                        80,
                        manifest::ArtifactStorageMode::Bundled,
                    ),
                ),
                (
                    loose,
                    manifest::ChunkIndexEntry::new(
                        60,
                        0,
                        loose.to_hex().into(),
                        4,
                        60,
                        manifest::ArtifactStorageMode::LooseChunks,
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
            (1, 124, 4 * 1024 * 1024 + 1),
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
            manifest::ArtifactStorageMode::LooseChunks
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
}
