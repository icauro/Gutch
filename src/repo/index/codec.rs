use std::collections::BTreeMap;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read;
use std::io::Write;
use std::path::Path;

use crate::Error;
use crate::Result;
use crate::artifact::hash::ChunkHash;
use crate::manifest;

use crate::artifact::format::CHUNK_INDEX_MAGIC as CHUNK_INDEX_BINARY_MAGIC;

use crate::artifact::binary_io::*;
pub(crate) fn load_chunk_index_binary(path: &Path) -> Result<manifest::ChunkIndex> {
    let file = std::fs::File::open(path)?;
    let remaining = file.metadata()?.len();
    let reader = BinaryReader {
        inner: BufReader::new(file),
        remaining,
    };
    read_chunk_index_binary(reader)
}

fn read_chunk_index_binary(mut reader: BinaryReader<impl Read>) -> Result<manifest::ChunkIndex> {
    if &read_exact_array::<16>(&mut reader)? != CHUNK_INDEX_BINARY_MAGIC {
        return Err(Error::manifest("invalid chunk index magic"));
    }
    if read_u16(&mut reader)? != 1 || read_u16(&mut reader)? != 0 {
        return Err(Error::manifest("unsupported chunk index version or flags"));
    }
    let count = read_u64(&mut reader)?;
    let artifact_count = read_u32(&mut reader)?;
    let updated_at = read_len_prefixed_string(&mut reader)?;
    reader.require_records(u64::from(artifact_count), 40)?;
    let mut artifacts = Vec::with_capacity(artifact_count as usize);
    let mut unique_artifacts = std::collections::HashSet::new();
    for _ in 0..artifact_count {
        let id = read_chunk_hash(&mut reader)?;
        let size = read_u32(&mut reader)?;
        let kind = read_u8(&mut reader)?;
        if read_exact_array::<3>(&mut reader)? != [0; 3]
            || kind != 0
            || size == 0
            || !unique_artifacts.insert(id)
        {
            return Err(Error::manifest("invalid or duplicate index artifact"));
        }
        artifacts.push((smol_str::SmolStr::new(id.to_hex()), size));
    }
    reader.require_records(count, 44)?;
    let mut entries = manifest::ChunkIndexEntries::new();
    let mut raw_totals = vec![0u64; artifacts.len()];
    let mut ordinals = std::collections::HashSet::new();
    let mut previous = None;
    for _ in 0..count {
        let hash = read_chunk_hash(&mut reader)?;
        if previous.is_some_and(|value| hash <= value) {
            return Err(Error::manifest(
                "chunk index keys must be unique and sorted",
            ));
        }
        previous = Some(hash);
        let artifact = read_u32(&mut reader)?;
        let ordinal_or_size = read_u32(&mut reader)?;
        let raw_size = u64::from(read_u32(&mut reader)?);
        if raw_size == 0 || raw_size > crate::artifact::format::MAX_RAW_CHUNK_SIZE {
            return Err(Error::manifest("invalid index chunk size"));
        }
        let (bundle_id, artifact_size, chunk_ordinal, storage_mode) = if artifact == u32::MAX {
            if ordinal_or_size == 0 {
                return Err(Error::manifest("invalid loose artifact size"));
            }
            (
                smol_str::SmolStr::new(hash.to_hex()),
                ordinal_or_size,
                0,
                manifest::ArtifactStorageMode::LooseChunks,
            )
        } else {
            if !ordinals.insert((artifact, ordinal_or_size)) {
                return Err(Error::manifest("duplicate chunk ordinal"));
            }
            let (id, size) = artifacts
                .get(artifact as usize)
                .ok_or_else(|| Error::manifest("invalid artifact row"))?;
            raw_totals[artifact as usize] = raw_totals[artifact as usize]
                .checked_add(raw_size)
                .ok_or_else(|| Error::manifest("index raw size overflow"))?;
            (
                id.clone(),
                *size,
                ordinal_or_size,
                manifest::ArtifactStorageMode::Bundled,
            )
        };
        entries.insert(
            hash,
            manifest::ChunkIndexEntry {
                bundle_id,
                raw_size: crate::artifact::format::size_u32(raw_size)?,
                compressed_size: artifact_size,
                storage_mode,
                artifact_size,
                chunk_ordinal,
            },
        );
    }
    if reader.remaining != 0 {
        return Err(Error::manifest("chunk index contains trailing bytes"));
    }
    let totals: BTreeMap<_, _> = artifacts
        .iter()
        .zip(raw_totals)
        .map(|((id, _), raw)| (id.as_str(), raw))
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
        updated_at,
        entries,
    })
}

pub(crate) fn load_chunk_index_binary_from_bytes(bytes: &[u8]) -> Result<manifest::ChunkIndex> {
    read_chunk_index_binary(BinaryReader {
        inner: bytes,
        remaining: bytes.len() as u64,
    })
}

pub(crate) fn write_chunk_index_binary(path: &Path, index: &manifest::ChunkIndex) -> Result<()> {
    let mut artifacts = BTreeMap::new();
    for entry in index.entries.values() {
        if entry.raw_size == 0
            || u64::from(entry.raw_size) > crate::artifact::format::MAX_RAW_CHUNK_SIZE
            || entry.artifact_size == 0
        {
            return Err(Error::manifest("invalid indexed artifact or chunk size"));
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
    crate::filesystem::write_atomic_with(path, |file| {
        let mut writer = BufWriter::new(file);
        writer.write_all(CHUNK_INDEX_BINARY_MAGIC)?;
        write_u16(&mut writer, 1)?;
        write_u16(&mut writer, 0)?;
        write_u64(&mut writer, index.entries.len() as u64)?;
        write_u32(&mut writer, artifact_count)?;
        write_len_prefixed_string(&mut writer, &index.updated_at)?;
        for (id, size) in &artifacts {
            write_chunk_hash(&mut writer, *id)?;
            write_u32(&mut writer, *size)?;
            writer.write_all(&[0; 4])?;
        }
        for (hash, entry) in &index.entries {
            write_chunk_hash(&mut writer, *hash)?;
            match entry.storage_mode {
                manifest::ArtifactStorageMode::Bundled => {
                    write_u32(
                        &mut writer,
                        artifact_rows[&ChunkHash::parse_hex(&entry.bundle_id)?],
                    )?;
                    write_u32(&mut writer, entry.chunk_ordinal)?;
                }
                manifest::ArtifactStorageMode::LooseChunks => {
                    write_u32(&mut writer, u32::MAX)?;
                    write_u32(&mut writer, entry.artifact_size)?;
                }
            }
            write_u32(
                &mut writer,
                crate::artifact::format::size_u32(entry.raw_size)?,
            )?;
        }
        writer.flush()?;
        Ok(())
    })
}
