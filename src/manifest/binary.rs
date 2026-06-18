use std::collections::BTreeMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::manifest::{self, FileManifest, target::TargetManifest};
use crate::{Error, Result};

const CHUNK_INDEX_BINARY_MAGIC: &[u8; 8] = b"GTCIDX1\0";
const CHUNK_INDEX_BINARY_VERSION: u16 = 3;
const TARGET_FILES_BINARY_MAGIC: &[u8; 8] = b"GTCTGT1\0";
const TARGET_FILES_BINARY_VERSION: u16 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TargetManifestShell {
    pub manifest_version: String,
    #[serde(alias = "created_at")]
    pub inserted_at: String,
    pub total_logical_size: u64,
    pub total_compressed_size_estimate: u64,
    #[serde(
        default,
        serialize_with = "crate::manifest::target::serialize_target_tags_as_compact_strings",
        deserialize_with = "crate::manifest::target::deserialize_target_tags_from_compact_strings"
    )]
    pub target_tags: manifest::TargetTagSet,
}

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
    for value in values {
        write_len_prefixed_string(writer, value)?;
    }
    Ok(())
}

fn read_len_prefixed_string_array(reader: &mut impl Read, count: usize) -> Result<Vec<String>> {
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(read_len_prefixed_string(reader)?);
    }
    Ok(values)
}

fn write_u32_array(writer: &mut impl Write, values: &[u32]) -> Result<()> {
    for value in values {
        write_u32(writer, *value)?;
    }
    Ok(())
}

fn read_u32_array(reader: &mut impl Read, count: usize) -> Result<Vec<u32>> {
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(read_u32(reader)?);
    }
    Ok(values)
}

fn write_u8_array(writer: &mut impl Write, values: &[u8]) -> Result<()> {
    for value in values {
        write_u8(writer, *value)?;
    }
    Ok(())
}

fn read_u8_array(reader: &mut impl Read, count: usize) -> Result<Vec<u8>> {
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(read_u8(reader)?);
    }
    Ok(values)
}

fn write_u64_array(writer: &mut impl Write, values: &[u64]) -> Result<()> {
    for value in values {
        write_u64(writer, *value)?;
    }
    Ok(())
}

fn read_u64_array(reader: &mut impl Read, count: usize) -> Result<Vec<u64>> {
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(read_u64(reader)?);
    }
    Ok(values)
}

fn read_bool_array(reader: &mut impl Read, count: usize) -> Result<Vec<bool>> {
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(read_exact_array::<1>(reader)?[0] != 0);
    }
    Ok(values)
}

fn write_bool_array(writer: &mut impl Write, values: &[bool]) -> Result<()> {
    for value in values {
        writer.write_all(&[u8::from(*value)])?;
    }
    Ok(())
}

pub(crate) fn target_manifest_shell_from_manifest(
    target_manifest: &TargetManifest,
) -> TargetManifestShell {
    TargetManifestShell {
        manifest_version: target_manifest.manifest_version.clone(),
        inserted_at: target_manifest.inserted_at.clone(),
        total_logical_size: target_manifest.total_logical_size,
        total_compressed_size_estimate: target_manifest.total_compressed_size_estimate,
        target_tags: target_manifest.target_tags.clone(),
    }
}

pub(crate) fn target_manifest_from_shell(
    shell: TargetManifestShell,
    target_id: String,
    files: Vec<FileManifest>,
) -> TargetManifest {
    TargetManifest {
        manifest_version: shell.manifest_version,
        target_id,
        inserted_at: shell.inserted_at,
        total_logical_size: shell.total_logical_size,
        total_compressed_size_estimate: shell.total_compressed_size_estimate,
        target_tags: shell.target_tags,
        files,
    }
}

pub(crate) fn load_chunk_index_binary(path: &Path) -> Result<manifest::ChunkIndex> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let magic = read_exact_array::<8>(&mut reader)?;
    if &magic != CHUNK_INDEX_BINARY_MAGIC {
        return Err(Error::manifest(format!(
            "invalid chunk index binary header in {}",
            path.display()
        )));
    }

    let version = read_u16(&mut reader)?;
    return match version {
        1 => load_chunk_index_binary_v1(path, reader),
        2 => load_chunk_index_binary_v2(path, reader),
        CHUNK_INDEX_BINARY_VERSION => load_chunk_index_binary_v3(path, reader),
        _ => Err(Error::manifest(format!(
            "unsupported chunk index binary version {} in {}",
            version,
            path.display()
        ))),
    };
}

fn load_chunk_index_binary_v1(
    path: &Path,
    mut reader: BufReader<std::fs::File>,
) -> Result<manifest::ChunkIndex> {
    let _reserved = read_u16(&mut reader)?;
    let bundle_count = read_u32(&mut reader)? as usize;
    let chunk_count = read_u64(&mut reader)? as usize;
    let format_version = read_len_prefixed_string(&mut reader)?;
    let updated_at = read_len_prefixed_string(&mut reader)?;

    let bundles = read_len_prefixed_string_array(&mut reader, bundle_count)?;
    let mut entries = BTreeMap::new();
    for _ in 0..chunk_count {
        let chunk_hash = read_len_prefixed_string(&mut reader)?;
        let bundle_index = read_u32(&mut reader)? as usize;
        let raw_size = read_u64(&mut reader)?;
        let compressed_size = read_u64(&mut reader)?;
        let bundle_id = bundles.get(bundle_index).cloned().ok_or_else(|| {
            Error::manifest(format!(
                "chunk index bundle reference out of range in {}",
                path.display()
            ))
        })?;
        entries.insert(
            chunk_hash,
            manifest::ChunkIndexEntry {
                bundle_id,
                raw_size,
                compressed_size,
                storage_mode: manifest::ArtifactStorageMode::Bundled,
            },
        );
    }

    Ok(manifest::ChunkIndex {
        format_version,
        updated_at,
        entries,
    })
}

fn load_chunk_index_binary_v2(
    path: &Path,
    mut reader: BufReader<std::fs::File>,
) -> Result<manifest::ChunkIndex> {
    let _reserved = read_u16(&mut reader)?;
    let bundle_count = read_u32(&mut reader)? as usize;
    let chunk_count = read_u64(&mut reader)? as usize;
    let format_version = read_len_prefixed_string(&mut reader)?;
    let updated_at = read_len_prefixed_string(&mut reader)?;

    let bundles = read_len_prefixed_string_array(&mut reader, bundle_count)?;
    let chunk_hashes = read_len_prefixed_string_array(&mut reader, chunk_count)?;
    let bundle_indexes = read_u32_array(&mut reader, chunk_count)?;
    let raw_sizes = read_u64_array(&mut reader, chunk_count)?;
    let compressed_sizes = read_u64_array(&mut reader, chunk_count)?;
    let mut entries = BTreeMap::new();
    for index in 0..chunk_count {
        let chunk_hash = chunk_hashes[index].clone();
        let bundle_index = bundle_indexes[index] as usize;
        let raw_size = raw_sizes[index];
        let compressed_size = compressed_sizes[index];
        let bundle_id = bundles.get(bundle_index).cloned().ok_or_else(|| {
            Error::manifest(format!(
                "chunk index bundle reference out of range in {}",
                path.display()
            ))
        })?;
        entries.insert(
            chunk_hash,
            manifest::ChunkIndexEntry {
                bundle_id,
                raw_size,
                compressed_size,
                storage_mode: manifest::ArtifactStorageMode::Bundled,
            },
        );
    }

    Ok(manifest::ChunkIndex {
        format_version,
        updated_at,
        entries,
    })
}

fn load_chunk_index_binary_v3(
    path: &Path,
    mut reader: BufReader<std::fs::File>,
) -> Result<manifest::ChunkIndex> {
    let _reserved = read_u16(&mut reader)?;
    let chunk_count = read_u64(&mut reader)? as usize;
    let format_version = read_len_prefixed_string(&mut reader)?;
    let updated_at = read_len_prefixed_string(&mut reader)?;

    let chunk_hashes = read_len_prefixed_string_array(&mut reader, chunk_count)?;
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
            chunk_hashes[index].clone(),
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
        chunk_hashes.push(chunk_hash.clone());
        storage_modes.push(match entry.storage_mode {
            manifest::ArtifactStorageMode::Bundled => 0,
            manifest::ArtifactStorageMode::LooseChunks => 1,
        });
        object_ids.push(entry.bundle_id.clone());
        raw_sizes.push(entry.raw_size);
        compressed_sizes.push(entry.compressed_size);
    }
    write_len_prefixed_string_array(&mut writer, &chunk_hashes)?;
    write_u8_array(&mut writer, &storage_modes)?;
    write_len_prefixed_string_array(&mut writer, &object_ids)?;
    write_u64_array(&mut writer, &raw_sizes)?;
    write_u64_array(&mut writer, &compressed_sizes)?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn write_target_manifest_files_binary(
    path: &Path,
    target_manifest: &TargetManifest,
) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(TARGET_FILES_BINARY_MAGIC)?;
    write_u16(&mut writer, TARGET_FILES_BINARY_VERSION)?;
    write_u16(&mut writer, 0)?;
    write_u32(&mut writer, target_manifest.files.len() as u32)?;
    let chunk_ref_count = target_manifest
        .files
        .iter()
        .map(|file_manifest| file_manifest.chunks.len() as u64)
        .sum::<u64>();
    write_u64(&mut writer, chunk_ref_count)?;

    let mut paths = Vec::with_capacity(target_manifest.files.len());
    let mut sizes = Vec::with_capacity(target_manifest.files.len());
    let mut executables = Vec::with_capacity(target_manifest.files.len());
    let mut full_file_hashes = Vec::with_capacity(target_manifest.files.len());
    let mut chunk_counts = Vec::with_capacity(target_manifest.files.len());
    let mut chunk_hashes = Vec::with_capacity(chunk_ref_count as usize);
    let mut chunk_raw_sizes = Vec::with_capacity(chunk_ref_count as usize);

    for file_manifest in &target_manifest.files {
        paths.push(file_manifest.path.clone());
        sizes.push(file_manifest.size);
        executables.push(file_manifest.executable);
        full_file_hashes.push(file_manifest.full_file_hash.clone());
        chunk_counts.push(file_manifest.chunks.len() as u32);
        for chunk_ref in &file_manifest.chunks {
            chunk_hashes.push(chunk_ref.chunk_hash.clone());
            chunk_raw_sizes.push(chunk_ref.raw_size);
        }
    }

    write_len_prefixed_string_array(&mut writer, &paths)?;
    write_u64_array(&mut writer, &sizes)?;
    write_bool_array(&mut writer, &executables)?;
    write_len_prefixed_string_array(&mut writer, &full_file_hashes)?;
    write_u32_array(&mut writer, &chunk_counts)?;
    write_len_prefixed_string_array(&mut writer, &chunk_hashes)?;
    write_u64_array(&mut writer, &chunk_raw_sizes)?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn load_target_manifest_files_binary(path: &Path) -> Result<Vec<FileManifest>> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let magic = read_exact_array::<8>(&mut reader)?;
    if &magic != TARGET_FILES_BINARY_MAGIC {
        return Err(Error::manifest(format!(
            "invalid target files binary header in {}",
            path.display()
        )));
    }

    let version = read_u16(&mut reader)?;
    return match version {
        1 => load_target_manifest_files_binary_v1(path, reader),
        TARGET_FILES_BINARY_VERSION => load_target_manifest_files_binary_v2(path, reader),
        _ => Err(Error::manifest(format!(
            "unsupported target files binary version {} in {}",
            version,
            path.display()
        ))),
    };
}

fn load_target_manifest_files_binary_v1(
    _path: &Path,
    mut reader: BufReader<std::fs::File>,
) -> Result<Vec<FileManifest>> {
    let _reserved = read_u16(&mut reader)?;
    let file_count = read_u32(&mut reader)? as usize;
    let mut files = Vec::with_capacity(file_count);
    for _ in 0..file_count {
        let path_text = read_len_prefixed_string(&mut reader)?;
        let size = read_u64(&mut reader)?;
        let executable = read_exact_array::<1>(&mut reader)?[0] != 0;
        let _padding = read_exact_array::<7>(&mut reader)?;
        let full_file_hash = read_len_prefixed_string(&mut reader)?;
        let chunk_count = read_u32(&mut reader)? as usize;
        let mut chunks = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            chunks.push(manifest::ChunkRef {
                chunk_hash: read_len_prefixed_string(&mut reader)?,
                raw_size: read_u64(&mut reader)?,
            });
        }
        files.push(FileManifest {
            path: path_text,
            size,
            executable,
            full_file_hash,
            chunks,
        });
    }

    Ok(files)
}

fn load_target_manifest_files_binary_v2(
    path: &Path,
    mut reader: BufReader<std::fs::File>,
) -> Result<Vec<FileManifest>> {
    let _reserved = read_u16(&mut reader)?;
    let file_count = read_u32(&mut reader)? as usize;
    let chunk_ref_count = read_u64(&mut reader)? as usize;
    let paths = read_len_prefixed_string_array(&mut reader, file_count)?;
    let sizes = read_u64_array(&mut reader, file_count)?;
    let executables = read_bool_array(&mut reader, file_count)?;
    let full_file_hashes = read_len_prefixed_string_array(&mut reader, file_count)?;
    let chunk_counts = read_u32_array(&mut reader, file_count)?;
    let chunk_hashes = read_len_prefixed_string_array(&mut reader, chunk_ref_count)?;
    let chunk_raw_sizes = read_u64_array(&mut reader, chunk_ref_count)?;

    let mut files = Vec::with_capacity(file_count);
    let mut chunk_start = 0usize;
    for file_index in 0..file_count {
        let chunk_count = chunk_counts[file_index] as usize;
        let chunk_end = chunk_start + chunk_count;
        if chunk_end > chunk_ref_count {
            return Err(Error::manifest(format!(
                "target files chunk range out of bounds in {}",
                path.display()
            )));
        }
        let mut chunks = Vec::with_capacity(chunk_count);
        for chunk_index in chunk_start..chunk_end {
            chunks.push(manifest::ChunkRef {
                chunk_hash: chunk_hashes[chunk_index].clone(),
                raw_size: chunk_raw_sizes[chunk_index],
            });
        }
        files.push(FileManifest {
            path: paths[file_index].clone(),
            size: sizes[file_index],
            executable: executables[file_index],
            full_file_hash: full_file_hashes[file_index].clone(),
            chunks,
        });
        chunk_start = chunk_end;
    }
    if chunk_start != chunk_ref_count {
        return Err(Error::manifest(format!(
            "target files chunk count mismatch in {}",
            path.display()
        )));
    }

    Ok(files)
}
