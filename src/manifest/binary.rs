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
use crate::manifest::snapshot::SnapshotBlueprint;

use crate::artifact::format::CHUNK_INDEX_MAGIC as CHUNK_INDEX_BINARY_MAGIC;
use crate::artifact::format::SNAPSHOT_BLUEPRINT_MAGIC as SNAPSHOT_FILES_BINARY_MAGIC;

const SNAPSHOT_FILES_BINARY_VERSION: u16 = 1;

struct BinaryReader<R> {
    inner: R,
    remaining: u64,
}

impl<R: Read> BinaryReader<R> {
    fn require_records(&self, count: u64, minimum_bytes: u64) -> Result<()> {
        if count > self.remaining / minimum_bytes {
            return Err(Error::manifest(
                "binary manifest declares more data than remains",
            ));
        }
        Ok(())
    }
}

impl<R: Read> Read for BinaryReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let limit = buffer
            .len()
            .min(self.remaining.min(usize::MAX as u64) as usize);
        let count = self.inner.read(&mut buffer[..limit])?;
        self.remaining -= count as u64;
        Ok(count)
    }
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

fn read_len_prefixed_string(reader: &mut BinaryReader<impl Read>) -> Result<String> {
    let length = read_u32(reader)? as usize;
    reader.require_records(length as u64, 1)?;
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|err| Error::manifest(format!("invalid UTF-8: {}", err)))
}

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

pub(crate) fn write_snapshot_blueprint_binary(
    path: &Path,
    snapshot_blueprint: &SnapshotBlueprint,
) -> Result<()> {
    manifest::validate_snapshot_entries(&snapshot_blueprint.entries)?;
    crate::filesystem::write_atomic_with(path, |file| {
        let mut writer = BufWriter::new(file);
        writer.write_all(SNAPSHOT_FILES_BINARY_MAGIC)?;
        write_u16(&mut writer, SNAPSHOT_FILES_BINARY_VERSION)?;
        write_u16(&mut writer, 0)?;
        write_u32(
            &mut writer,
            u32::try_from(snapshot_blueprint.entries.len())
                .map_err(|_| Error::manifest("too many blueprint entries"))?,
        )?;
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
                    write_u32(
                        &mut writer,
                        u32::try_from(file.chunks.len())
                            .map_err(|_| Error::manifest("too many file chunks"))?,
                    )?;
                    for chunk in &file.chunks {
                        writer.write_all(ChunkHash::parse_hex(&chunk.chunk_hash)?.as_bytes())?;
                        write_u32(
                            &mut writer,
                            crate::artifact::format::size_u32(chunk.raw_size)?,
                        )?;
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
    let remaining = file.metadata()?.len();
    read_snapshot_blueprint(BinaryReader {
        inner: BufReader::new(file),
        remaining,
    })
}

pub(crate) fn read_snapshot_blueprint_bytes(bytes: &[u8]) -> Result<Vec<manifest::SnapshotEntry>> {
    read_snapshot_blueprint(BinaryReader {
        inner: bytes,
        remaining: bytes.len() as u64,
    })
}

fn read_snapshot_blueprint(
    mut reader: BinaryReader<impl Read>,
) -> Result<Vec<manifest::SnapshotEntry>> {
    let magic = read_exact_array::<16>(&mut reader)?;
    if &magic != SNAPSHOT_FILES_BINARY_MAGIC {
        return Err(Error::manifest("invalid snapshot blueprint binary header"));
    }

    let version = read_u16(&mut reader)?;
    if version != SNAPSHOT_FILES_BINARY_VERSION {
        return Err(Error::manifest(format!(
            "unsupported snapshot blueprint binary version {}",
            version
        )));
    }

    if read_u16(&mut reader)? != 0 {
        return Err(Error::manifest("nonzero reserved manifest bits"));
    }
    let entry_count = read_u32(&mut reader)? as usize;
    reader.require_records(entry_count as u64, 5)?;
    let chunk_ref_count = read_u64(&mut reader)?;
    reader.require_records(chunk_ref_count, 36)?;
    let chunk_ref_count = usize::try_from(chunk_ref_count)
        .map_err(|_| Error::manifest("chunk count exceeds address space"))?;
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
                reader.require_records(count as u64, 36)?;
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
                        raw_size: read_u32(&mut reader)?,
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

    #[test]
    fn rejects_impossible_counts_and_string_lengths_before_allocation() {
        let mut blueprint = b"GUTCH_BLUEPRINT\0\x01\0\0\0".to_vec();
        write_u32(&mut blueprint, u32::MAX).unwrap();
        write_u64(&mut blueprint, 0).unwrap();
        assert!(read_snapshot_blueprint_bytes(&blueprint).is_err());
        let mut index = b"GUTCH_INDEX\0\0\0\0\0\x01\0\0\0".to_vec();
        write_u64(&mut index, u64::MAX).unwrap();
        assert!(load_chunk_index_binary_from_bytes(&index).is_err());
        let mut reader = BinaryReader {
            inner: &u32::MAX.to_le_bytes()[..],
            remaining: 4,
        };
        assert!(read_len_prefixed_string(&mut reader).is_err());
    }

    #[test]
    fn independent_index_fixture_rejects_padding_version_sizes_and_trailing_bytes() {
        let mut bytes = b"GUTCH_INDEX\0\0\0\0\0\x01\0\0\0".to_vec();
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[7; 32]);
        bytes.extend_from_slice(&200u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&[9; 32]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&128u32.to_le_bytes());
        assert_eq!(bytes.len(), 36 + 40 + 44);
        let index = load_chunk_index_binary_from_bytes(&bytes).unwrap();
        let entry = index.entries.get(&"09".repeat(32)).unwrap();
        assert_eq!(entry.bundle_id.as_str(), "07".repeat(32));
        assert_eq!(entry.artifact_size, 200);
        assert_eq!(entry.raw_size, 128);
        for offset in [0, 15, 16, 18, 72, 108] {
            let mut malformed = bytes.clone();
            malformed[offset] = 255;
            assert!(
                load_chunk_index_binary_from_bytes(&malformed).is_err(),
                "offset {offset}"
            );
        }
        let mut oversized = bytes.clone();
        oversized[116..120].copy_from_slice(&(4 * 1024 * 1024 + 1u32).to_le_bytes());
        assert!(load_chunk_index_binary_from_bytes(&oversized).is_err());
        bytes.push(0);
        assert!(load_chunk_index_binary_from_bytes(&bytes).is_err());
    }

    #[test]
    fn blueprint_preserves_file_sizes_above_u32_without_wide_chunk_sizes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("blueprint");
        let size = 1u64 << 32;
        let blueprint = SnapshotBlueprint {
            entries: vec![
                manifest::FileManifest {
                    path: "large.bin".into(),
                    size,
                    executable: false,
                    read_only: false,
                    full_file_hash: "01".repeat(32),
                    chunks: vec![
                        manifest::ChunkRef {
                            chunk_hash: "02".repeat(32),
                            raw_size: 4 * 1024 * 1024
                        };
                        1024
                    ],
                }
                .into(),
            ],
        };
        write_snapshot_blueprint_binary(&path, &blueprint).unwrap();
        let restored = read_snapshot_blueprint_binary(&path).unwrap();
        assert_eq!(restored[0].as_file().unwrap().size, size);
        assert_eq!(
            std::fs::metadata(path).unwrap().len(),
            32 + 51 + 9 + 36 * 1024
        );
    }

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
        assert_eq!(u16::from_le_bytes([bytes[16], bytes[17]]), 1);
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
        bytes[32] = 9;
        std::fs::write(&path, bytes).unwrap();
        assert!(read_snapshot_blueprint_binary(&path).is_err());
        let _ = std::fs::remove_file(path);
    }
}
