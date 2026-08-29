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
    crate::helpers::write_atomic_with(path, |file| {
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
    })
}

pub(crate) fn write_snapshot_blueprint_binary(
    path: &Path,
    snapshot_blueprint: &SnapshotBlueprint,
) -> Result<()> {
    manifest::validate_snapshot_entries(&snapshot_blueprint.entries)?;
    crate::helpers::write_atomic_with(path, |file| {
        let mut writer = BufWriter::new(file);
        writer.write_all(SNAPSHOT_FILES_BINARY_MAGIC)?;
        write_u16(&mut writer, SNAPSHOT_FILES_BINARY_VERSION)?;
        write_u16(&mut writer, 0)?;
        write_u32(&mut writer, snapshot_blueprint.entries.len() as u32)?;
        let chunk_ref_count = snapshot_blueprint
            .entries
            .iter()
            .filter_map(manifest::SnapshotEntry::as_file)
            .map(|file| file.chunks.len() as u64)
            .sum::<u64>();
        write_u64(&mut writer, chunk_ref_count)?;
        for entry in &snapshot_blueprint.entries {
            match entry {
                manifest::SnapshotEntry::File(file) => {
                    writer.write_all(&[0])?;
                    write_len_prefixed_string(&mut writer, &file.path)?;
                    write_u64(&mut writer, file.size)?;
                    writer.write_all(&[u8::from(file.executable), u8::from(file.read_only)])?;
                    writer.write_all(ChunkHash::parse_hex(&file.full_file_hash)?.as_bytes())?;
                    write_u32(&mut writer, file.chunks.len() as u32)?;
                    for chunk in &file.chunks {
                        writer.write_all(ChunkHash::parse_hex(&chunk.chunk_hash)?.as_bytes())?;
                        write_u64(&mut writer, chunk.raw_size)?;
                    }
                }
                manifest::SnapshotEntry::Directory(directory) => {
                    writer.write_all(&[1])?;
                    write_len_prefixed_string(&mut writer, &directory.path)?;
                    writer.write_all(&[u8::from(directory.read_only)])?;
                }
                manifest::SnapshotEntry::Symlink(link) => {
                    writer.write_all(&[2])?;
                    write_len_prefixed_string(&mut writer, &link.path)?;
                    write_len_prefixed_string(&mut writer, &link.target_path)?;
                }
            }
        }
        writer.flush()?;
        Ok(())
    })
}

pub(crate) fn read_snapshot_blueprint_binary(path: &Path) -> Result<Vec<manifest::SnapshotEntry>> {
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
    let entry_count = read_u32(&mut reader)? as usize;
    let chunk_ref_count = read_u64(&mut reader)? as usize;
    let mut read_chunk_count = 0usize;
    let mut entries = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let kind = read_u8(&mut reader)?;
        let path = read_len_prefixed_string(&mut reader)?;
        let entry = match kind {
            0 => {
                let size = read_u64(&mut reader)?;
                let executable = read_u8(&mut reader)?;
                let read_only = read_u8(&mut reader)?;
                if executable > 1 || read_only > 1 {
                    return Err(Error::manifest("snapshot blueprint flags must be 0 or 1"));
                }
                let full_file_hash = ChunkHash::from_bytes(read_exact_array::<32>(&mut reader)?);
                let count = read_u32(&mut reader)? as usize;
                read_chunk_count = read_chunk_count
                    .checked_add(count)
                    .ok_or_else(|| Error::manifest("snapshot blueprint chunk count overflow"))?;
                if read_chunk_count > chunk_ref_count {
                    return Err(Error::manifest(
                        "snapshot blueprint chunk count exceeds header",
                    ));
                }
                let mut chunks = Vec::with_capacity(count);
                for _ in 0..count {
                    chunks.push(manifest::ChunkRef {
                        chunk_hash: ChunkHash::from_bytes(read_exact_array::<32>(&mut reader)?)
                            .to_hex(),
                        raw_size: read_u64(&mut reader)?,
                    });
                }
                manifest::SnapshotEntry::File(manifest::FileManifest {
                    path,
                    size,
                    executable: executable != 0,
                    read_only: read_only != 0,
                    full_file_hash: full_file_hash.to_hex(),
                    chunks,
                })
            }
            1 => {
                let read_only = read_u8(&mut reader)?;
                if read_only > 1 {
                    return Err(Error::manifest("snapshot blueprint flags must be 0 or 1"));
                }
                manifest::SnapshotEntry::Directory(manifest::DirectoryManifest {
                    path,
                    read_only: read_only != 0,
                })
            }
            2 => manifest::SnapshotEntry::Symlink(manifest::SymlinkManifest {
                path,
                target_path: read_len_prefixed_string(&mut reader)?,
            }),
            value => {
                return Err(Error::manifest(format!(
                    "snapshot blueprint entry kind {value} is invalid"
                )));
            }
        };
        entries.push(entry);
    }
    if read_chunk_count != chunk_ref_count {
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
    manifest::validate_snapshot_entries(&entries)?;
    Ok(entries)
}

#[cfg(test)]
mod snapshot_blueprint_tests {
    use super::*;

    fn all_entries() -> Vec<manifest::SnapshotEntry> {
        vec![
            manifest::SnapshotEntry::Directory(manifest::DirectoryManifest {
                path: "empty".into(),
                read_only: false,
            }),
            manifest::SnapshotEntry::File(manifest::FileManifest {
                path: "game.bin".into(),
                size: 0,
                executable: true,
                read_only: true,
                full_file_hash: blake3::hash(b"").to_hex().to_string(),
                chunks: Vec::new(),
            }),
            manifest::SnapshotEntry::Symlink(manifest::SymlinkManifest {
                path: "current".into(),
                target_path: "empty".into(),
            }),
        ]
    }

    #[test]
    fn version_one_blueprint_round_trips_all_entry_kinds() {
        let path = crate::test_util::temp_path("gutch_entry_blueprint_roundtrip");
        let blueprint = SnapshotBlueprint {
            entries: all_entries(),
        };
        write_snapshot_blueprint_binary(&path, &blueprint).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 1);
        let loaded = read_snapshot_blueprint_binary(&path).unwrap();
        assert_eq!(loaded.len(), 3);
        assert!(matches!(loaded[0], manifest::SnapshotEntry::Directory(_)));
        assert!(matches!(loaded[1], manifest::SnapshotEntry::File(_)));
        assert!(matches!(loaded[2], manifest::SnapshotEntry::Symlink(_)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn blueprint_rejects_unknown_entry_kind() {
        let path = crate::test_util::temp_path("gutch_entry_blueprint_bad_kind");
        write_snapshot_blueprint_binary(
            &path,
            &SnapshotBlueprint {
                entries: all_entries(),
            },
        )
        .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[20] = 9;
        std::fs::write(&path, bytes).unwrap();
        assert!(read_snapshot_blueprint_binary(&path).is_err());
        let _ = std::fs::remove_file(path);
    }
}
