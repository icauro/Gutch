//! Snapshot blueprint columns, independent of compression framing.
//!
//! The three groups are paths, entry metadata, and explicit chunk sequences.
//! Each group stores whole columns consecutively, not one record at a time.
//! See _docs/current_state/15_binary_formats.md for the exact v1 wire layout.
//!
//! Path rows contain sorted ancestor directories followed by sorted entries.
//! Directory IDs are one-based; zero denotes the implicit root. Entry IDs are
//! zero-based and are used by symlink targets. Ancestors are path dictionary
//! rows, not additional filesystem entries.
//!
//! Metadata columns use entry order, except file-only and symlink-only columns.
//! Explicit chunk offsets use only files needing stored chunk references.
//! Reconstruction advances those separate row counters as it visits entries.
use crate::artifact::format::MAX_RAW_CHUNK_SIZE;
use crate::artifact::metadata::MAX_METADATA_BYTES;
use crate::artifact::{binary_io::*, hash::ChunkHash};
use crate::filesystem::paths::MAX_MANIFEST_PATH_BYTES;
use crate::filesystem::paths::validate_manifest_relative_path;
use crate::manifest::{
    self, ChunkRef, DirectoryManifest, FileManifest, SnapshotEntry, SymlinkManifest,
};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::str::from_utf8;

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
        // Validation ensures the sole chunk spans the file; matching hashes let
        // the reader recover the chunk entirely from the file size and hash.
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
    let mut entries_sorted_by_path: Vec<_> = entries.iter().collect();
    entries_sorted_by_path.sort_unstable_by_key(|entry| entry.path());
    let files: Vec<_> = entries_sorted_by_path
        .iter()
        .filter_map(|entry| entry.as_file())
        .collect();
    Ok(vec![
        encode_paths(&entries_sorted_by_path)?,
        encode_entry_metadata(&entries_sorted_by_path, &files)?,
        encode_chunk_sequences(&files)?,
    ])
}

fn encode_paths(entries_sorted_by_path: &[&SnapshotEntry]) -> Result<Vec<u8>> {
    let mut directories = BTreeSet::new();
    let mut expanded_path_bytes = 0usize;
    for entry in entries_sorted_by_path {
        expanded_path_bytes = expanded_path_bytes
            .checked_add(entry.path().len())
            .filter(|size| *size <= MAX_METADATA_BYTES)
            .ok_or_else(|| Error::manifest("expanded paths exceed metadata limit"))?;
        for (offset, _) in entry.path().match_indices('/') {
            if directories.insert(&entry.path()[..offset]) {
                if directories.len() > MAX_PATH_ROWS - entries_sorted_by_path.len() {
                    return Err(Error::manifest("blueprint exceeds 2097152 path rows"));
                }
                expanded_path_bytes = expanded_path_bytes
                    .checked_add(offset)
                    .filter(|size| *size <= MAX_METADATA_BYTES)
                    .ok_or_else(|| Error::manifest("expanded paths exceed metadata limit"))?;
            }
        }
    }
    // Lexical order places each ancestor before its descendants. Every parent
    // was inserted above, so the dictionary lookup below is complete.
    let directory_ids_by_path: BTreeMap<_, _> = directories
        .iter()
        .enumerate()
        .map(|(index, path)| (*path, index as u32 + 1))
        .collect();
    let parent_id = |parent: &str| {
        if parent.is_empty() {
            0
        } else {
            directory_ids_by_path[parent]
        }
    };
    let mut paths = Vec::new();
    write_u32(&mut paths, column_length_u32(directories.len())?)?;
    write_u32(&mut paths, column_length_u32(entries_sorted_by_path.len())?)?;
    let mut path_components = Vec::with_capacity(directories.len() + entries_sorted_by_path.len());
    for directory_path in &directories {
        path_components.push(split_path(directory_path));
    }
    for entry in entries_sorted_by_path {
        path_components.push(split_path(entry.path()));
    }
    for (parent, _) in &path_components {
        write_u32(&mut paths, parent_id(parent))?;
    }
    let mut offset = 0usize;
    write_u32(&mut paths, 0)?;
    for (_, leaf) in &path_components {
        offset = offset
            .checked_add(leaf.len())
            .ok_or_else(|| Error::manifest("name bytes overflow"))?;
        write_u32(&mut paths, column_length_u32(offset)?)?;
    }
    for (_, leaf) in &path_components {
        paths.extend_from_slice(leaf.as_bytes());
    }

    Ok(paths)
}

fn encode_entry_metadata(
    entries_sorted_by_path: &[&SnapshotEntry],
    files: &[&FileManifest],
) -> Result<Vec<u8>> {
    let mut entry_metadata = Vec::new();
    for entry in entries_sorted_by_path {
        entry_metadata.push(match entry {
            SnapshotEntry::File(_) => ENTRY_FILE,
            SnapshotEntry::Directory(_) => ENTRY_DIRECTORY,
            SnapshotEntry::Symlink(_) => ENTRY_SYMLINK,
        });
    }
    for entry in entries_sorted_by_path {
        let mut entry_flags = 0;
        match entry {
            SnapshotEntry::File(file) => {
                if file.read_only {
                    entry_flags |= FLAG_READ_ONLY;
                }
                if file.executable {
                    entry_flags |= FLAG_EXECUTABLE;
                }
            }
            SnapshotEntry::Directory(directory) => {
                if directory.read_only {
                    entry_flags |= FLAG_READ_ONLY;
                }
            }
            SnapshotEntry::Symlink(_) => {}
        }
        write_u16(&mut entry_metadata, entry_flags)?;
    }
    for file in files {
        write_u64(&mut entry_metadata, file.size)?;
    }
    for file in files {
        let hash = file.full_file_hash;
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
    let entry_ids_by_path: BTreeMap<_, _> = entries_sorted_by_path
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.path(), index as u32))
        .collect();
    for entry in entries_sorted_by_path {
        if let SnapshotEntry::Symlink(link) = entry {
            // encode() has validated that each symlink target is an entry.
            write_u32(
                &mut entry_metadata,
                entry_ids_by_path[link.target_path.as_str()],
            )?;
        }
    }

    Ok(entry_metadata)
}

fn encode_chunk_sequences(files: &[&FileManifest]) -> Result<Vec<u8>> {
    let files_with_explicit_chunks: Vec<_> = files
        .iter()
        .filter(|file| file_representation(file) == FILE_EXPLICIT_CHUNKS)
        .collect();
    let mut chunks = Vec::new();
    let mut offset = 0usize;
    write_u32(&mut chunks, 0)?;
    for file in &files_with_explicit_chunks {
        offset = offset
            .checked_add(file.chunks.len())
            .ok_or_else(|| Error::manifest("chunk offsets overflow"))?;
        write_u32(&mut chunks, column_length_u32(offset)?)?;
    }
    for file in &files_with_explicit_chunks {
        for chunk in &file.chunks {
            write_chunk_hash(&mut chunks, chunk.chunk_hash)?;
        }
    }
    for file in &files_with_explicit_chunks {
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
    let explicit_chunk_sequence_count = metadata
        .file_representations
        .iter()
        .filter(|&&representation| representation == FILE_EXPLICIT_CHUNKS)
        .count();
    let chunks = decode_chunk_sequences(chunk_group, explicit_chunk_sequence_count)?;
    reconstruct_entries(&entry_paths, metadata, chunks)
}

fn decode_paths(bytes: &[u8]) -> Result<Vec<String>> {
    let mut path_reader = reader(bytes);
    let directory_count = read_u32(&mut path_reader)? as usize;
    let entry_count = read_u32(&mut path_reader)? as usize;
    let path_row_count = directory_count
        .checked_add(entry_count)
        .ok_or_else(|| Error::manifest("path row overflow"))?;
    if path_row_count > MAX_PATH_ROWS {
        return Err(Error::manifest("blueprint exceeds 2097152 path rows"));
    }
    path_reader.require_records(path_row_count as u64, 8)?;
    let parent_directory_ids: Vec<_> = (0..path_row_count)
        .map(|_| read_u32(&mut path_reader))
        .collect::<Result<_>>()?;
    let leaf_byte_offsets: Vec<_> = (0..=path_row_count)
        .map(|_| read_u32(&mut path_reader))
        .collect::<Result<_>>()?;
    if leaf_byte_offsets.first() != Some(&0)
        || leaf_byte_offsets.last().copied().map(u64::from) != Some(path_reader.remaining)
    {
        return Err(Error::manifest("invalid name offsets"));
    }
    let leaf_name_bytes = path_reader.inner;
    let mut directory_paths = vec![String::new()];
    let mut entry_paths = Vec::with_capacity(entry_count);
    let mut expanded_path_bytes = 0usize;
    for row in 0..path_row_count {
        let parent_directory_id = parent_directory_ids[row] as usize;
        // A directory row may reference only root or an earlier directory row.
        // IDs are one-based, so parent_directory_id == row still refers backward.
        if parent_directory_id > directory_count
            || (row < directory_count && parent_directory_id > row)
        {
            return Err(Error::manifest("invalid directory parent or cycle"));
        }
        let leaf_byte_range = leaf_byte_offsets[row] as usize..leaf_byte_offsets[row + 1] as usize;
        let leaf_bytes = leaf_name_bytes
            .get(leaf_byte_range)
            .ok_or_else(|| Error::manifest("invalid UTF-8 leaf or name span"))?;
        let leaf_name = from_utf8(leaf_bytes)
            .map_err(|_| Error::manifest("invalid UTF-8 leaf or name span"))?;
        if leaf_name.is_empty() || leaf_name.contains('/') {
            return Err(Error::manifest("invalid UTF-8 leaf or name span"));
        }
        let parent_path = directory_paths
            .get(parent_directory_id)
            .ok_or_else(|| Error::manifest("invalid directory reference"))?;
        let path_length =
            parent_path.len() + usize::from(!parent_path.is_empty()) + leaf_name.len();
        expanded_path_bytes = expanded_path_bytes
            .checked_add(path_length)
            .filter(|size| *size <= MAX_METADATA_BYTES)
            .ok_or_else(|| Error::manifest("expanded paths exceed metadata limit"))?;
        if path_length > MAX_MANIFEST_PATH_BYTES {
            return Err(Error::manifest("expanded path exceeds path limit"));
        }
        let path = if parent_path.is_empty() {
            leaf_name.to_owned()
        } else {
            format!("{parent_path}/{leaf_name}")
        };
        validate_manifest_relative_path(&path, false)?;
        if row < directory_count {
            if directory_paths.last().is_some_and(|last| last >= &path) {
                return Err(Error::manifest("directory paths must be sorted and unique"));
            }
            directory_paths.push(path);
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
    entry_kinds: Vec<u8>,
    entry_flags: Vec<u16>,
    file_sizes: Vec<u64>,
    file_content_hashes: Vec<ChunkHash>,
    file_representations: Vec<u8>,
    symlink_target_entry_ids: Vec<usize>,
}

fn decode_entry_metadata(bytes: &[u8], entry_count: usize) -> Result<EntryMetadata> {
    let mut entry_metadata = reader(bytes);
    entry_metadata.require_records(entry_count as u64, 3)?;
    let entry_kinds: Vec<_> = (0..entry_count)
        .map(|_| read_u8(&mut entry_metadata))
        .collect::<Result<_>>()?;
    let entry_flags: Vec<_> = (0..entry_count)
        .map(|_| read_u16(&mut entry_metadata))
        .collect::<Result<_>>()?;
    for (&kind, &flag) in entry_kinds.iter().zip(&entry_flags) {
        let allowed_flags = match kind {
            ENTRY_FILE => FLAG_READ_ONLY | FLAG_EXECUTABLE,
            ENTRY_DIRECTORY => FLAG_READ_ONLY,
            ENTRY_SYMLINK => 0,
            _ => return Err(Error::manifest("invalid entry kind")),
        };
        if flag & !allowed_flags != 0 {
            return Err(Error::manifest("invalid entry flags"));
        }
    }
    let file_count = entry_kinds
        .iter()
        .filter(|&&kind| kind == ENTRY_FILE)
        .count();
    entry_metadata.require_records(file_count as u64, 9)?;
    let file_sizes: Vec<_> = (0..file_count)
        .map(|_| read_u64(&mut entry_metadata))
        .collect::<Result<_>>()?;
    let file_content_hashes: Vec<_> = file_sizes
        .iter()
        .map(|&size| {
            if size == 0 {
                Ok(ChunkHash::from_bytes(*blake3::hash(b"").as_bytes()))
            } else {
                read_chunk_hash(&mut entry_metadata)
            }
        })
        .collect::<Result<_>>()?;
    let file_representations: Vec<_> = (0..file_count)
        .map(|_| read_u8(&mut entry_metadata))
        .collect::<Result<_>>()?;
    let mut symlink_target_entry_ids = Vec::new();
    for &kind in &entry_kinds {
        if kind == ENTRY_SYMLINK {
            let target_entry_id = read_u32(&mut entry_metadata)? as usize;
            if target_entry_id >= entry_count {
                return Err(Error::manifest("invalid symlink target reference"));
            }
            symlink_target_entry_ids.push(target_entry_id);
        }
    }
    require_column_end(&entry_metadata)?;
    Ok(EntryMetadata {
        entry_kinds,
        entry_flags,
        file_sizes,
        file_content_hashes,
        file_representations,
        symlink_target_entry_ids,
    })
}

struct ChunkSequences<'a> {
    chunk_sequence_offsets: Vec<u32>,
    chunk_hashes: &'a [u8],
    chunk_sizes: &'a [u8],
}

fn decode_chunk_sequences(
    bytes: &[u8],
    explicit_chunk_sequence_count: usize,
) -> Result<ChunkSequences<'_>> {
    let mut chunks = reader(bytes);
    chunks.require_records(explicit_chunk_sequence_count as u64 + 1, 4)?;
    let chunk_sequence_offsets: Vec<_> = (0..=explicit_chunk_sequence_count)
        .map(|_| read_u32(&mut chunks))
        .collect::<Result<_>>()?;
    let chunk_count = chunk_sequence_offsets.last().copied().unwrap_or(0) as usize;
    // Each explicit sequence is nonempty; the final offset is the chunk count.
    let has_empty_or_reversed_sequence = chunk_sequence_offsets
        .windows(2)
        .any(|pair| pair[0] >= pair[1]);
    let expected_chunk_column_bytes = chunk_count as u64 * (32 + 4);
    if chunk_sequence_offsets[0] != 0
        || has_empty_or_reversed_sequence
        || chunks.remaining != expected_chunk_column_bytes
    {
        return Err(Error::manifest("invalid chunk sequence offsets"));
    }
    // The exact column-length check above bounds this split. Keep the decoded
    // group's bytes borrowed until reconstruction materializes each file's chunks.
    let (chunk_hashes, chunk_sizes) = chunks.inner.split_at(chunk_count * 32);
    Ok(ChunkSequences {
        chunk_sequence_offsets,
        chunk_hashes,
        chunk_sizes,
    })
}

fn reconstruct_entries(
    entry_paths: &[String],
    metadata: EntryMetadata,
    chunks: ChunkSequences<'_>,
) -> Result<Vec<SnapshotEntry>> {
    let EntryMetadata {
        entry_kinds,
        entry_flags,
        file_sizes,
        file_content_hashes,
        file_representations,
        symlink_target_entry_ids,
    } = metadata;
    let ChunkSequences {
        chunk_sequence_offsets,
        mut chunk_hashes,
        mut chunk_sizes,
    } = chunks;
    let entry_count = entry_paths.len();
    let mut entries = Vec::with_capacity(entry_count);
    // Counts and target bounds were checked while decoding each column group.
    let (mut file_row, mut symlink_row, mut explicit_chunk_sequence_row) = (0, 0, 0);
    for (entry_row, path) in entry_paths.iter().enumerate() {
        entries.push(match entry_kinds[entry_row] {
            ENTRY_FILE => {
                let size = file_sizes[file_row];
                let hash = file_content_hashes[file_row];
                let file_chunks = match file_representations[file_row] {
                    FILE_EMPTY if size == 0 => Vec::new(),
                    FILE_IMPLICIT_CHUNK if size > 0 && size <= MAX_RAW_CHUNK_SIZE => {
                        vec![ChunkRef {
                            chunk_hash: file_content_hashes[file_row],
                            raw_size: size as u32,
                        }]
                    }
                    FILE_EXPLICIT_CHUNKS if size > 0 => {
                        let chunk_count = (chunk_sequence_offsets[explicit_chunk_sequence_row + 1]
                            - chunk_sequence_offsets[explicit_chunk_sequence_row])
                            as usize;
                        explicit_chunk_sequence_row += 1;
                        let mut file_chunks = Vec::with_capacity(chunk_count);
                        for _ in 0..chunk_count {
                            file_chunks.push(ChunkRef {
                                chunk_hash: read_chunk_hash(&mut chunk_hashes)?,
                                raw_size: read_u32(&mut chunk_sizes)?,
                            });
                        }
                        file_chunks
                    }
                    _ => return Err(Error::manifest("invalid file representation")),
                };
                file_row += 1;
                SnapshotEntry::File(FileManifest {
                    path: path.clone(),
                    size,
                    full_file_hash: hash,
                    chunks: file_chunks,
                    read_only: entry_flags[entry_row] & FLAG_READ_ONLY != 0,
                    executable: entry_flags[entry_row] & FLAG_EXECUTABLE != 0,
                })
            }
            ENTRY_DIRECTORY => SnapshotEntry::Directory(DirectoryManifest {
                path: path.clone(),
                read_only: entry_flags[entry_row] & FLAG_READ_ONLY != 0,
            }),
            _ => {
                let target_path = entry_paths[symlink_target_entry_ids[symlink_row]].clone();
                symlink_row += 1;
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
    fn mixed_file_representations_preserve_separate_chunk_sequences() {
        let files: [(&str, &[&[u8]]); 4] = [
            ("a-multi", &[b"ab", b"cde", b"f"]),
            ("b-single", &[b"one"]),
            ("c-empty", &[]),
            ("d-multi", &[b"gh", b"ijk"]),
        ];
        let mut entries = Vec::new();
        for (path, parts) in files {
            let bytes = parts.concat();
            entries.push(SnapshotEntry::File(FileManifest {
                path: path.into(),
                size: bytes.len() as u64,
                executable: true,
                read_only: false,
                full_file_hash: ChunkHash::of(&bytes),
                chunks: parts
                    .iter()
                    .map(|part| ChunkRef {
                        chunk_hash: ChunkHash::of(part),
                        raw_size: part.len() as u32,
                    })
                    .collect(),
            }));
        }
        entries.push(SnapshotEntry::Directory(DirectoryManifest {
            path: "b-directory".into(),
            read_only: true,
        }));
        entries.push(SnapshotEntry::Symlink(SymlinkManifest {
            path: "c-link".into(),
            target_path: "a-multi".into(),
        }));
        entries.sort_by(|left, right| left.path().cmp(right.path()));
        let groups = encode(&entries).unwrap();
        let restored = decode(&groups).unwrap();
        assert_eq!(
            serde_json::to_value(&restored).unwrap(),
            serde_json::to_value(&entries).unwrap()
        );
        for length in 0..groups[2].len() {
            let mut truncated = groups.clone();
            truncated[2].truncate(length);
            assert!(decode(&truncated).is_err(), "chunk group length {length}");
        }
    }

    #[test]
    fn rejects_invalid_topology_flags_spans_and_representations() {
        let entries = vec![SnapshotEntry::File(FileManifest {
            path: "parent/child/file".into(),
            size: 1,
            executable: true,
            read_only: true,
            full_file_hash: ChunkHash::of(b"x"),
            chunks: vec![ChunkRef {
                chunk_hash: ChunkHash::of(b"x"),
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
