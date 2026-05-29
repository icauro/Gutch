use std::collections::BTreeMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::manifest::{self, FileManifest, target::TargetManifest};
use crate::{Error, Result};

const CHUNK_INDEX_BINARY_MAGIC: &[u8; 8] = b"GTCIDX1\0";
const CHUNK_INDEX_BINARY_VERSION: u16 = 1;
const TARGET_FILES_BINARY_MAGIC: &[u8; 8] = b"GTCTGT1\0";
const TARGET_FILES_BINARY_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TargetManifestShell {
    pub manifest_version: String,
    #[serde(alias = "created_at")]
    pub inserted_at: String,
    pub total_logical_size: u64,
    pub total_compressed_size_estimate: u64,
    #[serde(default)]
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
    if version != CHUNK_INDEX_BINARY_VERSION {
        return Err(Error::manifest(format!(
            "unsupported chunk index binary version {} in {}",
            version,
            path.display()
        )));
    }

    let _reserved = read_u16(&mut reader)?;
    let bundle_count = read_u32(&mut reader)? as usize;
    let chunk_count = read_u64(&mut reader)? as usize;
    let format_version = read_len_prefixed_string(&mut reader)?;
    let updated_at = read_len_prefixed_string(&mut reader)?;

    let mut bundles = Vec::with_capacity(bundle_count);
    for _ in 0..bundle_count {
        bundles.push(read_len_prefixed_string(&mut reader)?);
    }

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
            },
        );
    }

    Ok(manifest::ChunkIndex {
        format_version,
        updated_at,
        entries,
    })
}

pub(crate) fn write_chunk_index_binary(
    path: &Path,
    index: &manifest::ChunkIndex,
) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(CHUNK_INDEX_BINARY_MAGIC)?;
    write_u16(&mut writer, CHUNK_INDEX_BINARY_VERSION)?;
    write_u16(&mut writer, 0)?;

    let mut bundle_ids = Vec::new();
    let mut bundle_indexes = std::collections::HashMap::new();
    for entry in index.entries.values() {
        if !bundle_indexes.contains_key(&entry.bundle_id) {
            let next_index = bundle_ids.len() as u32;
            bundle_indexes.insert(entry.bundle_id.clone(), next_index);
            bundle_ids.push(entry.bundle_id.clone());
        }
    }

    write_u32(&mut writer, bundle_ids.len() as u32)?;
    write_u64(&mut writer, index.entries.len() as u64)?;
    write_len_prefixed_string(&mut writer, &index.format_version)?;
    write_len_prefixed_string(&mut writer, &index.updated_at)?;
    for bundle_id in &bundle_ids {
        write_len_prefixed_string(&mut writer, bundle_id)?;
    }

    for (chunk_hash, entry) in &index.entries {
        write_len_prefixed_string(&mut writer, chunk_hash)?;
        let bundle_index = bundle_indexes
            .get(&entry.bundle_id)
            .copied()
            .ok_or_else(|| Error::manifest("chunk index bundle table missing entry"))?;
        write_u32(&mut writer, bundle_index)?;
        write_u64(&mut writer, entry.raw_size)?;
        write_u64(&mut writer, entry.compressed_size)?;
    }
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
    for file_manifest in &target_manifest.files {
        write_len_prefixed_string(&mut writer, &file_manifest.path)?;
        write_u64(&mut writer, file_manifest.size)?;
        writer.write_all(&[u8::from(file_manifest.executable)])?;
        writer.write_all(&[0u8; 7])?;
        write_len_prefixed_string(&mut writer, &file_manifest.full_file_hash)?;
        write_u32(&mut writer, file_manifest.chunks.len() as u32)?;
        for chunk_ref in &file_manifest.chunks {
            write_len_prefixed_string(&mut writer, &chunk_ref.chunk_hash)?;
            write_u64(&mut writer, chunk_ref.raw_size)?;
        }
    }
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
    if version != TARGET_FILES_BINARY_VERSION {
        return Err(Error::manifest(format!(
            "unsupported target files binary version {} in {}",
            version,
            path.display()
        )));
    }

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
