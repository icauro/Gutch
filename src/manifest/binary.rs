use std::collections::BTreeMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::hash::ChunkHash;
use crate::manifest::{self, snapshot::SnapshotBlueprint};
use crate::{Error, Result};

const CHUNK_INDEX_BINARY_MAGIC: &[u8; 4] = b"GTCI";
const CHUNK_INDEX_BINARY_VERSION: u16 = 1;
const SNAPSHOT_FILES_BINARY_MAGIC: &[u8; 4] = b"GTCT";
const SNAPSHOT_FILES_BINARY_VERSION: u16 = 1;

fn read_exact_array<const N: usize>(reader: &mut impl Read) -> Result<[u8; N]> {
    let mut buffer = [0u8; N];
    reader.read_exact(&mut buffer)?;
    Ok(buffer)
}

fn read_u16(reader: &mut impl Read) -> Result<u16> {
    Ok(u16::from_le_bytes(read_exact_array(reader)?))
}

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    Ok(u32::from_le_bytes(read_exact_array(reader)?))
}

fn read_u8(reader: &mut impl Read) -> Result<u8> {
    Ok(read_exact_array::<1>(reader)?[0])
}

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    Ok(u64::from_le_bytes(read_exact_array(reader)?))
}

fn read_chunk_hash(reader: &mut impl Read) -> Result<ChunkHash> {
    Ok(ChunkHash::from_bytes(read_exact_array(reader)?))
}

fn write_u16(writer: &mut impl Write, value: u16) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_u32(writer: &mut impl Write, value: u32) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_u8(writer: &mut impl Write, value: u8) -> Result<()> {
    writer.write_all(&[value])?;
    Ok(())
}

fn write_u64(writer: &mut impl Write, value: u64) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_chunk_hash(writer: &mut impl Write, value: ChunkHash) -> Result<()> {
    writer.write_all(value.as_bytes())?;
    Ok(())
}

fn write_len_prefixed_string(writer: &mut impl Write, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let length = u32::try_from(bytes.len())
        .map_err(|_| Error::manifest("string too large for binary manifest"))?;
    write_u32(writer, length)?;
    writer.write_all(bytes)?;
    Ok(())
}

fn read_len_prefixed_string(reader: &mut impl Read) -> Result<String> {
    let length = read_u32(reader)? as usize;
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|err| Error::manifest(format!("invalid UTF-8: {}", err)))
}

fn write_len_prefixed_string_array(writer: &mut impl Write, values: &[String]) -> Result<()> {
    values
        .iter()
        .try_for_each(|value| write_len_prefixed_string(writer, value))
}

fn read_len_prefixed_string_array(reader: &mut impl Read, count: usize) -> Result<Vec<String>> {
    (0..count)
        .map(|_| read_len_prefixed_string(reader))
        .collect()
}

fn write_u32_array(writer: &mut impl Write, values: &[u32]) -> Result<()> {
    values
        .iter()
        .try_for_each(|value| write_u32(writer, *value))
}

fn read_u32_array(reader: &mut impl Read, count: usize) -> Result<Vec<u32>> {
    (0..count).map(|_| read_u32(reader)).collect()
}

fn write_u8_array(writer: &mut impl Write, values: &[u8]) -> Result<()> {
    values.iter().try_for_each(|value| write_u8(writer, *value))
}

fn read_u8_array(reader: &mut impl Read, count: usize) -> Result<Vec<u8>> {
    (0..count).map(|_| read_u8(reader)).collect()
}

fn write_u64_array(writer: &mut impl Write, values: &[u64]) -> Result<()> {
    values
        .iter()
        .try_for_each(|value| write_u64(writer, *value))
}

fn read_u64_array(reader: &mut impl Read, count: usize) -> Result<Vec<u64>> {
    (0..count).map(|_| read_u64(reader)).collect()
}

fn write_chunk_hash_array(writer: &mut impl Write, values: &[ChunkHash]) -> Result<()> {
    values
        .iter()
        .try_for_each(|value| write_chunk_hash(writer, *value))
}

fn read_chunk_hash_array(reader: &mut impl Read, count: usize) -> Result<Vec<ChunkHash>> {
    (0..count).map(|_| read_chunk_hash(reader)).collect()
}

fn write_bool_array(writer: &mut impl Write, values: &[bool]) -> Result<()> {
    values
        .iter()
        .try_for_each(|value| write_u8(writer, u8::from(*value)))
}

pub(crate) fn load_chunk_index_binary(path: &Path) -> Result<manifest::ChunkIndex> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let magic = read_exact_array::<4>(&mut reader)?;
    if &magic != CHUNK_INDEX_BINARY_MAGIC {
        return Err(Error::manifest(format!(
            "invalid chunk index binary header in {}",
            path.display()
        )));
    }

    let version = read_u16(&mut reader)?;
    if version != CHUNK_INDEX_BINARY_VERSION {
        return Err(Error::manifest(format!(
            "unsupported chunk index binary version {} in {}",
            version,
            path.display()
        )));
    }

    let _reserved = read_u16(&mut reader)?;
    let chunk_count = read_u64(&mut reader)? as usize;
    let format_version = read_len_prefixed_string(&mut reader)?;
    let updated_at = read_len_prefixed_string(&mut reader)?;

    let chunk_hashes = read_chunk_hash_array(&mut reader, chunk_count)?;
    let storage_modes = read_u8_array(&mut reader, chunk_count)?;
    let object_ids = read_len_prefixed_string_array(&mut reader, chunk_count)?;
    let raw_sizes = read_u64_array(&mut reader, chunk_count)?;
    let compressed_sizes = read_u64_array(&mut reader, chunk_count)?;
    let mut entries = BTreeMap::new();
    for index in 0..chunk_count {
        let storage_mode = match storage_modes[index] {
            0 => manifest::ArtifactStorageMode::Bundled,
            1 => manifest::ArtifactStorageMode::LooseChunks,
            other => {
                return Err(Error::manifest(format!(
                    "invalid chunk index storage mode {} in {}",
                    other,
                    path.display()
                )));
            }
        };
        entries.insert(
            chunk_hashes[index].to_hex(),
            manifest::ChunkIndexEntry {
                bundle_id: object_ids[index].clone(),
                raw_size: raw_sizes[index],
                compressed_size: compressed_sizes[index],
                storage_mode,
            },
        );
    }

    Ok(manifest::ChunkIndex {
        format_version,
        updated_at,
        entries,
    })
}

pub(crate) fn write_chunk_index_binary(path: &Path, index: &manifest::ChunkIndex) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(CHUNK_INDEX_BINARY_MAGIC)?;
    write_u16(&mut writer, CHUNK_INDEX_BINARY_VERSION)?;
    write_u16(&mut writer, 0)?;

    write_u64(&mut writer, index.entries.len() as u64)?;
    write_len_prefixed_string(&mut writer, &index.format_version)?;
    write_len_prefixed_string(&mut writer, &index.updated_at)?;

    let mut chunk_hashes = Vec::with_capacity(index.entries.len());
    let mut storage_modes = Vec::with_capacity(index.entries.len());
    let mut object_ids = Vec::with_capacity(index.entries.len());
    let mut raw_sizes = Vec::with_capacity(index.entries.len());
    let mut compressed_sizes = Vec::with_capacity(index.entries.len());
    for (chunk_hash, entry) in &index.entries {
        chunk_hashes.push(ChunkHash::parse_hex(chunk_hash)?);
        storage_modes.push(match entry.storage_mode {
            manifest::ArtifactStorageMode::Bundled => 0,
            manifest::ArtifactStorageMode::LooseChunks => 1,
        });
        object_ids.push(entry.bundle_id.clone());
        raw_sizes.push(entry.raw_size);
        compressed_sizes.push(entry.compressed_size);
    }
    write_chunk_hash_array(&mut writer, &chunk_hashes)?;
    write_u8_array(&mut writer, &storage_modes)?;
    write_len_prefixed_string_array(&mut writer, &object_ids)?;
    write_u64_array(&mut writer, &raw_sizes)?;
    write_u64_array(&mut writer, &compressed_sizes)?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn write_snapshot_blueprint_binary(
    path: &Path,
    snapshot_blueprint: &SnapshotBlueprint,
) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(SNAPSHOT_FILES_BINARY_MAGIC)?;
    write_u16(&mut writer, SNAPSHOT_FILES_BINARY_VERSION)?;
    write_u16(&mut writer, 0)?;
    write_u32(&mut writer, snapshot_blueprint.files.len() as u32)?;
    let chunk_ref_count = snapshot_blueprint
        .files
        .iter()
        .map(|file_manifest| file_manifest.chunks.len() as u64)
        .sum::<u64>();
    write_u64(&mut writer, chunk_ref_count)?;

    let mut paths = Vec::with_capacity(snapshot_blueprint.files.len());
    let mut sizes = Vec::with_capacity(snapshot_blueprint.files.len());
    let mut executables = Vec::with_capacity(snapshot_blueprint.files.len());
    let mut full_file_hashes = Vec::with_capacity(snapshot_blueprint.files.len());
    let mut chunk_counts = Vec::with_capacity(snapshot_blueprint.files.len());
    let mut chunk_hashes = Vec::with_capacity(chunk_ref_count as usize);
    let mut chunk_raw_sizes = Vec::with_capacity(chunk_ref_count as usize);

    for file_manifest in &snapshot_blueprint.files {
        paths.push(file_manifest.path.clone());
        sizes.push(file_manifest.size);
        executables.push(file_manifest.executable);
        full_file_hashes.push(ChunkHash::parse_hex(&file_manifest.full_file_hash)?);
        chunk_counts.push(file_manifest.chunks.len() as u32);
        for chunk_ref in &file_manifest.chunks {
            chunk_hashes.push(ChunkHash::parse_hex(&chunk_ref.chunk_hash)?);
            chunk_raw_sizes.push(chunk_ref.raw_size);
        }
    }

    write_len_prefixed_string_array(&mut writer, &paths)?;
    write_u64_array(&mut writer, &sizes)?;
    write_bool_array(&mut writer, &executables)?;
    write_chunk_hash_array(&mut writer, &full_file_hashes)?;
    write_u32_array(&mut writer, &chunk_counts)?;
    write_chunk_hash_array(&mut writer, &chunk_hashes)?;
    write_u64_array(&mut writer, &chunk_raw_sizes)?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn read_snapshot_blueprint_binary(path: &Path) -> Result<Vec<manifest::FileManifest>> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let magic = read_exact_array::<4>(&mut reader)?;
    if &magic != SNAPSHOT_FILES_BINARY_MAGIC {
        return Err(Error::manifest(format!(
            "invalid snapshot blueprint binary header in {}",
            path.display()
        )));
    }

    let version = read_u16(&mut reader)?;
    if version != SNAPSHOT_FILES_BINARY_VERSION {
        return Err(Error::manifest(format!(
            "unsupported snapshot blueprint binary version {} in {}",
            version,
            path.display()
        )));
    }

    let _reserved = read_u16(&mut reader)?;
    let file_count = read_u32(&mut reader)? as usize;
    let chunk_ref_count = read_u64(&mut reader)? as usize;
    let paths = read_len_prefixed_string_array(&mut reader, file_count)?;
    let sizes = read_u64_array(&mut reader, file_count)?;
    let executables = read_u8_array(&mut reader, file_count)?;
    let full_file_hashes = read_chunk_hash_array(&mut reader, file_count)?;
    let chunk_counts = read_u32_array(&mut reader, file_count)?;
    let chunk_hashes = read_chunk_hash_array(&mut reader, chunk_ref_count)?;
    let chunk_raw_sizes = read_u64_array(&mut reader, chunk_ref_count)?;

    let mut chunk_offset = 0usize;
    let mut files = Vec::with_capacity(file_count);
    for index in 0..file_count {
        crate::validate_manifest_relative_path(&paths[index], false)?;
        let chunk_count = chunk_counts[index] as usize;
        let end = chunk_offset
            .checked_add(chunk_count)
            .ok_or_else(|| Error::manifest("snapshot blueprint chunk count overflow"))?;
        if end > chunk_hashes.len() {
            return Err(Error::manifest(
                "snapshot blueprint chunk count exceeds payload",
            ));
        }
        if executables[index] > 1 {
            return Err(Error::manifest(
                "snapshot blueprint executable flags must be 0 or 1",
            ));
        }
        let chunks: Vec<manifest::ChunkRef> = (chunk_offset..end)
            .map(|chunk_index| manifest::ChunkRef {
                chunk_hash: chunk_hashes[chunk_index].to_hex(),
                raw_size: chunk_raw_sizes[chunk_index],
            })
            .collect();
        let chunk_bytes = chunks.iter().map(|chunk| chunk.raw_size).sum::<u64>();
        // A local manifest can be staged before chunking completes. Treat an
        // empty chunk list as that intermediate state; completed blueprints
        // still have to account for the full file size.
        if chunk_count > 0 && chunk_bytes != sizes[index] {
            return Err(Error::manifest(format!(
                "snapshot blueprint file '{}' size does not match its chunk references",
                paths[index]
            )));
        }
        chunk_offset = end;
        files.push(manifest::FileManifest {
            path: paths[index].clone(),
            size: sizes[index],
            executable: executables[index] != 0,
            full_file_hash: full_file_hashes[index].to_hex(),
            chunks,
        });
    }
    if chunk_offset != chunk_ref_count {
        return Err(Error::manifest(
            "snapshot blueprint contains unreferenced chunk records",
        ));
    }
    let mut trailing = [0u8; 1];
    if reader.read(&mut trailing)? != 0 {
        return Err(Error::manifest(
            "snapshot blueprint contains trailing bytes",
        ));
    }
    Ok(files)
}
