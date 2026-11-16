//! Binary encoding of the chunk index.

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
use crate::artifact::format::MAX_RAW_BUNDLE_SIZE;
use crate::artifact::format::MAX_RAW_CHUNK_SIZE;
use crate::artifact::metadata;
use crate::artifact::metadata::read_file;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::filesystem::write_atomic_with;
use crate::repo::index::ChunkIndex;
use crate::repo::index::ChunkIndexEntries;
use crate::repo::index::ChunkIndexEntry;
use std::collections::HashSet;
use std::sync::Arc;

const LOOSE_CHUNK_ARTIFACT_ROW: u32 = u32::MAX;

pub(crate) fn load_chunk_index_binary(path: &Path) -> Result<ChunkIndex> {
    load_chunk_index_binary_from_bytes(&read_file(path)?)
}

fn decode_artifact_table(bytes: &[u8]) -> Result<Vec<Arc<super::records::IndexedArtifact>>> {
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
            return Err(Error::artifact("invalid or unsorted index artifact"));
        }
        previous_artifact_id = Some(artifact_id);
        artifacts.push(Arc::new(super::records::IndexedArtifact {
            bundle_id: std::sync::Arc::<str>::from(artifact_id.to_hex()),
            artifact_size,
            storage_mode: ArtifactStorageMode::Bundled,
        }));
    }
    if reader.remaining != 0 {
        return Err(Error::artifact("trailing artifact column bytes"));
    }
    Ok(artifacts)
}

fn read_chunk_index_binary(groups: &[Vec<u8>]) -> Result<ChunkIndex> {
    let artifacts = decode_artifact_table(&groups[0])?;
    let mut reader = BinaryReader {
        inner: groups[1].as_slice(),
        remaining: groups[1].len() as u64,
    };
    let chunk_count = read_u32(&mut reader)?;
    reader.require_records(u64::from(chunk_count), 44)?;
    let row_count = chunk_count as usize;
    // The 44-byte row preflight bounds all four column splits, including their products.
    let (mut chunk_hashes, remaining_columns) = reader.inner.split_at(row_count * 32);
    let (mut artifact_table_rows, remaining_columns) = remaining_columns.split_at(row_count * 4);
    // Loose chunks have no artifact-table row: their identity is the chunk hash,
    // and this column carries encoded size instead of a bundle ordinal.
    let (mut chunk_ordinals_or_artifact_sizes, remaining_columns) =
        remaining_columns.split_at(row_count * 4);
    let (mut chunk_raw_sizes, trailing_bytes) = remaining_columns.split_at(row_count * 4);
    let mut rows = Vec::with_capacity(row_count);
    let mut bundle_raw_byte_totals = vec![0u64; artifacts.len()];
    let mut seen_bundle_chunk_ordinals = HashSet::new();
    for _ in 0..row_count {
        let chunk_hash = read_chunk_hash(&mut chunk_hashes)?;
        let artifact_table_row = read_u32(&mut artifact_table_rows)?;
        let chunk_ordinal_or_artifact_size = read_u32(&mut chunk_ordinals_or_artifact_sizes)?;
        let chunk_raw_size = read_u32(&mut chunk_raw_sizes)?;
        let raw_size = u64::from(chunk_raw_size);
        if raw_size == 0 || raw_size > MAX_RAW_CHUNK_SIZE {
            return Err(Error::artifact("invalid index chunk size"));
        }
        let (record, chunk_ordinal) = if artifact_table_row == LOOSE_CHUNK_ARTIFACT_ROW {
            if chunk_ordinal_or_artifact_size == 0 {
                return Err(Error::artifact("invalid loose artifact size"));
            }
            (
                Arc::new(super::records::IndexedArtifact {
                    bundle_id: std::sync::Arc::<str>::from(chunk_hash.to_hex()),
                    artifact_size: chunk_ordinal_or_artifact_size,
                    storage_mode: ArtifactStorageMode::LooseChunks,
                }),
                0,
            )
        } else {
            if !seen_bundle_chunk_ordinals
                .insert((artifact_table_row, chunk_ordinal_or_artifact_size))
            {
                return Err(Error::artifact("duplicate chunk ordinal"));
            }
            let record = artifacts
                .get(artifact_table_row as usize)
                .ok_or_else(|| Error::artifact("invalid artifact row"))?;
            bundle_raw_byte_totals[artifact_table_row as usize] = bundle_raw_byte_totals
                [artifact_table_row as usize]
                .checked_add(raw_size)
                .filter(|bundle_raw_bytes| *bundle_raw_bytes <= MAX_RAW_BUNDLE_SIZE)
                .ok_or_else(|| Error::artifact("indexed bundle exceeds raw size limit"))?;
            (record.clone(), chunk_ordinal_or_artifact_size)
        };
        rows.push((
            chunk_hash,
            ChunkIndexEntry {
                artifact: record,
                chunk_ordinal,
                raw_size: chunk_raw_size,
            },
        ));
    }
    if !trailing_bytes.is_empty() {
        return Err(Error::artifact("chunk index contains trailing bytes"));
    }
    let entries = ChunkIndexEntries::from_sorted_rows(rows)?;
    Ok(ChunkIndex { entries })
}

pub(crate) fn load_chunk_index_binary_from_bytes(bytes: &[u8]) -> Result<ChunkIndex> {
    read_chunk_index_binary(&metadata::decode(bytes, CHUNK_INDEX_BINARY_MAGIC, 2)?)
}

pub(crate) fn write_chunk_index_binary(path: &Path, index: &ChunkIndex) -> Result<()> {
    let mut artifacts = BTreeMap::new();
    for (hash, entry) in &index.entries {
        if entry.raw_size == 0
            || u64::from(entry.raw_size) > MAX_RAW_CHUNK_SIZE
            || entry.artifact_size == 0
        {
            return Err(Error::artifact("invalid indexed artifact or chunk size"));
        }
        if entry.storage_mode == ArtifactStorageMode::LooseChunks
            && (ChunkHash::parse_hex(&entry.bundle_id)? != *hash || entry.chunk_ordinal != 0)
        {
            return Err(Error::artifact(
                "loose index identity or ordinal is inconsistent",
            ));
        }
        if entry.storage_mode == ArtifactStorageMode::Bundled {
            let id = entry.bundle_id.as_ref();
            if let Some((_, size)) = artifacts.get(id) {
                if *size != entry.artifact_size {
                    return Err(Error::artifact("inconsistent indexed artifact sizes"));
                }
            } else {
                // Canonical hex sorts in the same order as its binary hash.
                // Validate and decode each distinct artifact identity once.
                artifacts.insert(id, (ChunkHash::parse_hex(id)?, entry.artifact_size));
            }
        }
    }
    let artifact_count =
        u32::try_from(artifacts.len()).map_err(|_| Error::artifact("too many index artifacts"))?;
    if artifact_count == LOOSE_CHUNK_ARTIFACT_ROW {
        return Err(Error::artifact("artifact index sentinel exhausted"));
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
        u32::try_from(index.entries.len()).map_err(|_| Error::artifact("too many index chunks"))?,
    )?;
    for hash in index.entries.keys() {
        write_chunk_hash(&mut chunk_group, *hash)?;
    }
    for entry in index.entries.values() {
        write_u32(
            &mut chunk_group,
            match entry.storage_mode {
                ArtifactStorageMode::Bundled => artifact_rows[entry.bundle_id.as_ref()],
                ArtifactStorageMode::LooseChunks => LOOSE_CHUNK_ARTIFACT_ROW,
            },
        )?;
    }
    for entry in index.entries.values() {
        write_u32(
            &mut chunk_group,
            match entry.storage_mode {
                ArtifactStorageMode::Bundled => entry.chunk_ordinal,
                ArtifactStorageMode::LooseChunks => entry.artifact_size,
            },
        )?;
    }
    for entry in index.entries.values() {
        write_u32(&mut chunk_group, entry.raw_size)?;
    }
    let bytes = metadata::encode(CHUNK_INDEX_BINARY_MAGIC, &[artifact_group, chunk_group])?;
    // Apply exactly the reader's invariants before publishing writer input.
    load_chunk_index_binary_from_bytes(&bytes)?;
    write_atomic_with(path, |file| {
        file.write_all(&bytes)?;
        Ok(())
    })
}

#[cfg(test)]
#[path = "binary_codec_tests.rs"]
mod tests;
