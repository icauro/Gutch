//! Snapshot blueprint columns, independent of compression framing.
//!
//! The three groups are paths, entry metadata, and explicit chunk sequences.
//! Each group stores whole columns consecutively, not one record at a time.
//! See docs/current_state/15_binary_formats.md for the exact v1 wire layout.
//!
//! Path rows contain sorted ancestor directories followed by sorted entries.
//! Directory IDs are one-based; zero denotes the implicit root. Entry IDs are
//! zero-based and are used by symlink targets. Ancestors are path dictionary
//! rows, not additional filesystem entries.
//!
//! Metadata columns use entry order, except file-only and symlink-only columns.
//! Explicit chunk offsets use only files needing stored chunk references.
//! Reconstruction advances those separate row counters as it visits entries.
use crate::artifact::{binary_io::*, hash::ChunkHash};
use crate::manifest::{
    self, ChunkRef, DirectoryManifest, FileManifest, SnapshotEntry, SymlinkManifest,
};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

// A format bound, independent of pointer width or Rust enum layout.
const MAX_PATH_ROWS: usize = 1 << 21;

const ENTRY_FILE: u8 = 0;
const ENTRY_DIRECTORY: u8 = 1;
const ENTRY_SYMLINK: u8 = 2;
const FILE_EMPTY: u8 = 0;
const FILE_IMPLICIT_CHUNK: u8 = 1;
const FILE_EXPLICIT_CHUNKS: u8 = 2;
const FLAG_READ_ONLY: u16 = 1;
const FLAG_EXECUTABLE: u16 = 2;

fn file_representation(file: &FileManifest) -> u8 {
    match file.chunks.len() {
        0 => FILE_EMPTY,
        1 if file.chunks[0].chunk_hash == file.full_file_hash => FILE_IMPLICIT_CHUNK,
        _ => FILE_EXPLICIT_CHUNKS,
    }
}

fn column_length_u32(value: usize) -> Result<u32> {
    u32::try_from(value).map_err(|_| Error::manifest("blueprint column exceeds u32 range"))
}

fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

pub(crate) fn encode(entries: &[SnapshotEntry]) -> Result<Vec<Vec<u8>>> {
    if entries.len() > MAX_PATH_ROWS {
        return Err(Error::manifest("blueprint exceeds 2097152 path rows"));
    }
    manifest::validate_snapshot_entries(entries)?;
    let mut ordered: Vec<_> = entries.iter().collect();
    ordered.sort_unstable_by_key(|entry| entry.path());
    let files: Vec<_> = ordered.iter().filter_map(|entry| entry.as_file()).collect();
    Ok(vec![
        encode_paths(&ordered)?,
        encode_entry_metadata(&ordered, &files)?,
        encode_chunk_sequences(&files)?,
    ])
}

// Callers validate entries first: all ancestor and symlink target lookups exist.
fn encode_paths(ordered: &[&SnapshotEntry]) -> Result<Vec<u8>> {
    let mut directories = BTreeSet::new();
    let mut expanded_bytes = 0usize;
    for entry in ordered {
        expanded_bytes = expanded_bytes
            .checked_add(entry.path().len())
            .filter(|size| *size <= crate::artifact::metadata::MAX_METADATA_BYTES)
            .ok_or_else(|| Error::manifest("expanded paths exceed metadata limit"))?;
        for (offset, _) in entry.path().match_indices('/') {
            if directories.insert(&entry.path()[..offset]) {
                if directories.len() > MAX_PATH_ROWS - ordered.len() {
                    return Err(Error::manifest("blueprint exceeds 2097152 path rows"));
                }
                expanded_bytes = expanded_bytes
                    .checked_add(offset)
                    .filter(|size| *size <= crate::artifact::metadata::MAX_METADATA_BYTES)
                    .ok_or_else(|| Error::manifest("expanded paths exceed metadata limit"))?;
            }
        }
    }
    let directory_ids: BTreeMap<_, _> = directories
        .iter()
        .enumerate()
        .map(|(index, path)| (*path, index as u32 + 1))
        .collect();
    let parent_id = |parent: &str| {
        if parent.is_empty() {
            0
        } else {
            directory_ids[parent]
        }
    };
    let mut paths = Vec::new();
    write_u32(&mut paths, column_length_u32(directories.len())?)?;
    write_u32(&mut paths, column_length_u32(ordered.len())?)?;
    let rows: Vec<_> = directories
        .iter()
        .map(|path| split_path(path))
        .chain(ordered.iter().map(|entry| split_path(entry.path())))
        .collect();
    for (parent, _) in &rows {
        write_u32(&mut paths, parent_id(parent))?;
    }
    let mut offset = 0usize;
    write_u32(&mut paths, 0)?;
    for (_, leaf) in &rows {
        offset = offset
            .checked_add(leaf.len())
            .ok_or_else(|| Error::manifest("name bytes overflow"))?;
        write_u32(&mut paths, column_length_u32(offset)?)?;
    }
    for (_, leaf) in &rows {
        paths.extend_from_slice(leaf.as_bytes());
    }

    Ok(paths)
}

fn encode_entry_metadata(ordered: &[&SnapshotEntry], files: &[&FileManifest]) -> Result<Vec<u8>> {
    let mut entry_metadata = Vec::new();
    for entry in ordered {
        entry_metadata.push(match entry {
            SnapshotEntry::File(_) => ENTRY_FILE,
            SnapshotEntry::Directory(_) => ENTRY_DIRECTORY,
            SnapshotEntry::Symlink(_) => ENTRY_SYMLINK,
        });
    }
    for entry in ordered {
        write_u16(
            &mut entry_metadata,
            match entry {
                SnapshotEntry::File(file) => {
                    u16::from(file.read_only) * FLAG_READ_ONLY
                        | u16::from(file.executable) * FLAG_EXECUTABLE
                }
                SnapshotEntry::Directory(directory) => {
                    u16::from(directory.read_only) * FLAG_READ_ONLY
                }
                SnapshotEntry::Symlink(_) => 0,
            },
        )?;
    }
    for file in files {
        write_u64(&mut entry_metadata, file.size)?;
    }
    for file in files {
        let hash = ChunkHash::parse_hex(&file.full_file_hash)?;
        if file.size == 0 {
            if hash.as_bytes() != blake3::hash(b"").as_bytes() {
                return Err(Error::manifest("invalid empty file hash"));
            }
        } else {
            entry_metadata.extend_from_slice(hash.as_bytes());
        }
    }
    for file in files {
        entry_metadata.push(file_representation(file));
    }
    let entry_ids: BTreeMap<_, _> = ordered
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.path(), index as u32))
        .collect();
    for entry in ordered {
        if let SnapshotEntry::Symlink(link) = entry {
            write_u32(&mut entry_metadata, entry_ids[link.target_path.as_str()])?;
        }
    }

    Ok(entry_metadata)
}

fn encode_chunk_sequences(files: &[&FileManifest]) -> Result<Vec<u8>> {
    let explicit_files: Vec<_> = files
        .iter()
        .filter(|file| file_representation(file) == FILE_EXPLICIT_CHUNKS)
        .collect();
    let mut chunks = Vec::new();
    let mut offset = 0usize;
    write_u32(&mut chunks, 0)?;
    for file in &explicit_files {
        offset = offset
            .checked_add(file.chunks.len())
            .ok_or_else(|| Error::manifest("chunk offsets overflow"))?;
        write_u32(&mut chunks, column_length_u32(offset)?)?;
    }
    for file in &explicit_files {
        for chunk in &file.chunks {
            write_chunk_hash(&mut chunks, ChunkHash::parse_hex(&chunk.chunk_hash)?)?;
        }
    }
    for file in &explicit_files {
        for chunk in &file.chunks {
            write_u32(&mut chunks, chunk.raw_size)?;
        }
    }
    Ok(chunks)
}

fn reader(bytes: &[u8]) -> BinaryReader<&[u8]> {
    BinaryReader {
        inner: bytes,
        remaining: bytes.len() as u64,
    }
}
fn require_column_end(reader: &BinaryReader<&[u8]>) -> Result<()> {
    if reader.remaining != 0 {
        return Err(Error::manifest("trailing blueprint column bytes"));
    }
    Ok(())
}

pub(crate) fn decode(groups: &[Vec<u8>]) -> Result<Vec<SnapshotEntry>> {
    let [path_group, metadata_group, chunk_group] = groups else {
        return Err(Error::manifest("invalid blueprint groups"));
    };
    let entry_paths = decode_paths(path_group)?;
    let metadata = decode_entry_metadata(metadata_group, entry_paths.len())?;
    let explicit_count = metadata
        .representations
        .iter()
        .filter(|&&representation| representation == FILE_EXPLICIT_CHUNKS)
        .count();
    let chunks = decode_chunk_sequences(chunk_group, explicit_count)?;
    reconstruct_entries(&entry_paths, metadata, chunks)
}

fn decode_paths(bytes: &[u8]) -> Result<Vec<String>> {
    let mut paths = reader(bytes);
    let directory_count = read_u32(&mut paths)? as usize;
    let entry_count = read_u32(&mut paths)? as usize;
    let row_count = directory_count
        .checked_add(entry_count)
        .ok_or_else(|| Error::manifest("path row overflow"))?;
    if row_count > MAX_PATH_ROWS {
        return Err(Error::manifest("blueprint exceeds 2097152 path rows"));
    }
    paths.require_records(row_count as u64, 8)?;
    let parents: Vec<_> = (0..row_count)
        .map(|_| read_u32(&mut paths))
        .collect::<Result<_>>()?;
    let offsets: Vec<_> = (0..=row_count)
        .map(|_| read_u32(&mut paths))
        .collect::<Result<_>>()?;
    if offsets.first() != Some(&0)
        || offsets.last().copied().map(u64::from) != Some(paths.remaining)
    {
        return Err(Error::manifest("invalid name offsets"));
    }
    let names = paths.inner;
    let mut directories = vec![String::new()];
    let mut entry_paths = Vec::with_capacity(entry_count);
    let mut expanded_bytes = 0usize;
    for row in 0..row_count {
        let parent = parents[row] as usize;
        if parent > directory_count || (row < directory_count && parent > row) {
            return Err(Error::manifest("invalid directory parent or cycle"));
        }
        let leaf = names
            .get(offsets[row] as usize..offsets[row + 1] as usize)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .filter(|name| !name.is_empty() && !name.contains('/'))
            .ok_or_else(|| Error::manifest("invalid UTF-8 leaf or name span"))?;
        let parent = directories
            .get(parent)
            .ok_or_else(|| Error::manifest("invalid directory reference"))?;
        let path_length = parent.len() + usize::from(!parent.is_empty()) + leaf.len();
        expanded_bytes = expanded_bytes
            .checked_add(path_length)
            .filter(|size| *size <= crate::artifact::metadata::MAX_METADATA_BYTES)
            .ok_or_else(|| Error::manifest("expanded paths exceed metadata limit"))?;
        if path_length > crate::filesystem::paths::MAX_MANIFEST_PATH_BYTES {
            return Err(Error::manifest("expanded path exceeds path limit"));
        }
        let path = if parent.is_empty() {
            leaf.to_owned()
        } else {
            format!("{parent}/{leaf}")
        };
        crate::filesystem::paths::validate_manifest_relative_path(&path, false)?;
        if row < directory_count {
            if directories.last().is_some_and(|last| last >= &path) {
                return Err(Error::manifest("directory paths must be sorted and unique"));
            }
            directories.push(path);
        } else {
            if entry_paths.last().is_some_and(|last| last >= &path) {
                return Err(Error::manifest("entry paths must be sorted and unique"));
            }
            entry_paths.push(path);
        }
    }
    Ok(entry_paths)
}

struct EntryMetadata {
    kinds: Vec<u8>,
    flags: Vec<u16>,
    sizes: Vec<u64>,
    hashes: Vec<ChunkHash>,
    representations: Vec<u8>,
    targets: Vec<usize>,
}

fn decode_entry_metadata(bytes: &[u8], entry_count: usize) -> Result<EntryMetadata> {
    let mut entry_metadata = reader(bytes);
    entry_metadata.require_records(entry_count as u64, 3)?;
    let kinds: Vec<_> = (0..entry_count)
        .map(|_| read_u8(&mut entry_metadata))
        .collect::<Result<_>>()?;
    let flags: Vec<_> = (0..entry_count)
        .map(|_| read_u16(&mut entry_metadata))
        .collect::<Result<_>>()?;
    for (&kind, &flag) in kinds.iter().zip(&flags) {
        let allowed = match kind {
            ENTRY_FILE => FLAG_READ_ONLY | FLAG_EXECUTABLE,
            ENTRY_DIRECTORY => FLAG_READ_ONLY,
            ENTRY_SYMLINK => 0,
            _ => return Err(Error::manifest("invalid entry kind")),
        };
        if flag & !allowed != 0 {
            return Err(Error::manifest("invalid entry flags"));
        }
    }
    let file_count = kinds.iter().filter(|&&kind| kind == ENTRY_FILE).count();
    entry_metadata.require_records(file_count as u64, 9)?;
    let sizes: Vec<_> = (0..file_count)
        .map(|_| read_u64(&mut entry_metadata))
        .collect::<Result<_>>()?;
    let hashes: Vec<_> = sizes
        .iter()
        .map(|&size| {
            if size == 0 {
                Ok(ChunkHash::from_bytes(*blake3::hash(b"").as_bytes()))
            } else {
                read_chunk_hash(&mut entry_metadata)
            }
        })
        .collect::<Result<_>>()?;
    let representations: Vec<_> = (0..file_count)
        .map(|_| read_u8(&mut entry_metadata))
        .collect::<Result<_>>()?;
    let mut targets = Vec::new();
    for &kind in &kinds {
        if kind == ENTRY_SYMLINK {
            let target = read_u32(&mut entry_metadata)? as usize;
            if target >= entry_count {
                return Err(Error::manifest("invalid symlink target reference"));
            }
            targets.push(target);
        }
    }
    require_column_end(&entry_metadata)?;
    Ok(EntryMetadata {
        kinds,
        flags,
        sizes,
        hashes,
        representations,
        targets,
    })
}

struct ChunkSequences {
    offsets: Vec<u32>,
    hashes: Vec<ChunkHash>,
    sizes: Vec<u32>,
}

fn decode_chunk_sequences(bytes: &[u8], explicit_count: usize) -> Result<ChunkSequences> {
    let mut chunks = reader(bytes);
    chunks.require_records(explicit_count as u64 + 1, 4)?;
    let offsets: Vec<_> = (0..=explicit_count)
        .map(|_| read_u32(&mut chunks))
        .collect::<Result<_>>()?;
    let count = offsets.last().copied().unwrap_or(0) as usize;
    if offsets[0] != 0
        || offsets.windows(2).any(|pair| pair[0] >= pair[1])
        || chunks.remaining != count as u64 * 36
    {
        return Err(Error::manifest("invalid chunk sequence offsets"));
    }
    let chunk_hashes: Vec<_> = (0..count)
        .map(|_| read_chunk_hash(&mut chunks))
        .collect::<Result<_>>()?;
    let chunk_sizes: Vec<_> = (0..count)
        .map(|_| read_u32(&mut chunks))
        .collect::<Result<_>>()?;
    require_column_end(&chunks)?;
    Ok(ChunkSequences {
        offsets,
        hashes: chunk_hashes,
        sizes: chunk_sizes,
    })
}

fn reconstruct_entries(
    entry_paths: &[String],
    metadata: EntryMetadata,
    chunks: ChunkSequences,
) -> Result<Vec<SnapshotEntry>> {
    let EntryMetadata {
        kinds,
        flags,
        sizes,
        hashes,
        representations,
        targets,
    } = metadata;
    let ChunkSequences {
        offsets,
        hashes: chunk_hashes,
        sizes: chunk_sizes,
    } = chunks;
    let entry_count = entry_paths.len();
    let mut entries = Vec::with_capacity(entry_count);
    // Counts and target bounds were checked while decoding each column group.
    let (mut file_row, mut link_row, mut explicit_row) = (0, 0, 0);
    for (row, path) in entry_paths.iter().enumerate() {
        entries.push(match kinds[row] {
            ENTRY_FILE => {
                let size = sizes[file_row];
                let hash = hashes[file_row].to_hex();
                let file_chunks = match representations[file_row] {
                    FILE_EMPTY if size == 0 => Vec::new(),
                    FILE_IMPLICIT_CHUNK
                        if size > 0 && size <= crate::artifact::format::MAX_RAW_CHUNK_SIZE =>
                    {
                        vec![ChunkRef {
                            chunk_hash: hash.clone(),
                            raw_size: size as u32,
                        }]
                    }
                    FILE_EXPLICIT_CHUNKS if size > 0 => {
                        let range =
                            offsets[explicit_row] as usize..offsets[explicit_row + 1] as usize;
                        explicit_row += 1;
                        range
                            .map(|index| ChunkRef {
                                chunk_hash: chunk_hashes[index].to_hex(),
                                raw_size: chunk_sizes[index],
                            })
                            .collect()
                    }
                    _ => return Err(Error::manifest("invalid file representation")),
                };
                file_row += 1;
                SnapshotEntry::File(FileManifest {
                    path: path.clone(),
                    size,
                    full_file_hash: hash,
                    chunks: file_chunks,
                    read_only: flags[row] & FLAG_READ_ONLY != 0,
                    executable: flags[row] & FLAG_EXECUTABLE != 0,
                })
            }
            ENTRY_DIRECTORY => SnapshotEntry::Directory(DirectoryManifest {
                path: path.clone(),
                read_only: flags[row] & FLAG_READ_ONLY != 0,
            }),
            _ => {
                let target_path = entry_paths[targets[link_row]].clone();
                link_row += 1;
                SnapshotEntry::Symlink(SymlinkManifest {
                    path: path.clone(),
                    target_path,
                })
            }
        });
    }
    manifest::validate_snapshot_entries(&entries)?;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_topology_flags_spans_and_representations() {
        let entries = vec![SnapshotEntry::File(FileManifest {
            path: "parent/child/file".into(),
            size: 1,
            executable: true,
            read_only: true,
            full_file_hash: blake3::hash(b"x").to_hex().to_string(),
            chunks: vec![ChunkRef {
                chunk_hash: blake3::hash(b"x").to_hex().to_string(),
                raw_size: 1,
            }],
        })];
        let valid = encode(&entries).unwrap();
        assert!(decode(&valid).is_ok());
        for (description, group, offset, value) in [
            ("self-referencing directory", 0, 8, 1),
            ("cyclic directory parent", 0, 12, 2),
            ("nonzero first name offset", 0, 20, 1),
            ("unknown entry kind", 1, 0, 9),
            ("reserved low flag bit", 1, 1, 4),
            ("reserved high flag bit", 1, 2, 1),
            ("nonempty file with empty representation", 1, 43, 0),
            ("nonzero first chunk offset", 2, 0, 1),
        ] {
            let mut invalid = valid.clone();
            invalid[group][offset] = value;
            assert!(
                decode(&invalid).is_err(),
                "{description}: group {group}, offset {offset}"
            );
        }
        for group in 0..3 {
            for length in 0..valid[group].len() {
                let mut invalid = valid.clone();
                invalid[group].truncate(length);
                assert!(decode(&invalid).is_err(), "group {group}, length {length}");
            }
        }
    }
}
