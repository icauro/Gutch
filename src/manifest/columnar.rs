//! Snapshot blueprint columns, independent of compression framing.
use crate::artifact::{binary_io::*, hash::ChunkHash};
use crate::manifest::{
    self, ChunkRef, DirectoryManifest, FileManifest, SnapshotEntry, SymlinkManifest,
};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

// A format bound, independent of pointer width or Rust enum layout.
const MAX_PATH_ROWS: usize = 1 << 21;

fn narrow(value: usize) -> Result<u32> {
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
    let mut directories = BTreeSet::new();
    let mut expanded_bytes = 0usize;
    for entry in &ordered {
        expanded_bytes = expanded_bytes
            .checked_add(entry.path().len())
            .filter(|size| *size <= crate::artifact::metadata::MAX_METADATA_BYTES)
            .ok_or_else(|| Error::manifest("expanded paths exceed metadata limit"))?;
        for (offset, _) in entry.path().match_indices('/') {
            if directories.insert(&entry.path()[..offset]) {
                if directories.len() > MAX_PATH_ROWS - entries.len() {
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
    write_u32(&mut paths, narrow(directories.len())?)?;
    write_u32(&mut paths, narrow(ordered.len())?)?;
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
        write_u32(&mut paths, narrow(offset)?)?;
    }
    for (_, leaf) in &rows {
        paths.extend_from_slice(leaf.as_bytes());
    }

    let mut core = Vec::new();
    for entry in &ordered {
        core.push(match entry {
            SnapshotEntry::File(_) => 0,
            SnapshotEntry::Directory(_) => 1,
            SnapshotEntry::Symlink(_) => 2,
        });
    }
    for entry in &ordered {
        write_u16(
            &mut core,
            match entry {
                SnapshotEntry::File(file) => {
                    u16::from(file.read_only) | (u16::from(file.executable) << 1)
                }
                SnapshotEntry::Directory(directory) => u16::from(directory.read_only),
                SnapshotEntry::Symlink(_) => 0,
            },
        )?;
    }
    let files: Vec<_> = ordered.iter().filter_map(|entry| entry.as_file()).collect();
    for file in &files {
        write_u64(&mut core, file.size)?;
    }
    for file in &files {
        let hash = ChunkHash::parse_hex(&file.full_file_hash)?;
        if file.size == 0 {
            if hash.as_bytes() != blake3::hash(b"").as_bytes() {
                return Err(Error::manifest("invalid empty file hash"));
            }
        } else {
            core.extend_from_slice(hash.as_bytes());
        }
    }
    for file in &files {
        core.push(match file.chunks.len() {
            0 => 0,
            1 if file.chunks[0].chunk_hash == file.full_file_hash => 1,
            _ => 2,
        });
    }
    let entry_ids: BTreeMap<_, _> = ordered
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.path(), index as u32))
        .collect();
    for entry in &ordered {
        if let SnapshotEntry::Symlink(link) = entry {
            write_u32(&mut core, entry_ids[link.target_path.as_str()])?;
        }
    }

    let explicit: Vec<_> = files
        .iter()
        .filter(|file| {
            !file.chunks.is_empty()
                && !(file.chunks.len() == 1 && file.chunks[0].chunk_hash == file.full_file_hash)
        })
        .collect();
    let mut chunks = Vec::new();
    let mut offset = 0usize;
    write_u32(&mut chunks, 0)?;
    for file in &explicit {
        offset = offset
            .checked_add(file.chunks.len())
            .ok_or_else(|| Error::manifest("chunk offsets overflow"))?;
        write_u32(&mut chunks, narrow(offset)?)?;
    }
    for file in &explicit {
        for chunk in &file.chunks {
            write_chunk_hash(&mut chunks, ChunkHash::parse_hex(&chunk.chunk_hash)?)?;
        }
    }
    for file in &explicit {
        for chunk in &file.chunks {
            write_u32(&mut chunks, chunk.raw_size)?;
        }
    }
    Ok(vec![paths, core, chunks])
}

fn reader(bytes: &[u8]) -> BinaryReader<&[u8]> {
    BinaryReader {
        inner: bytes,
        remaining: bytes.len() as u64,
    }
}
fn finished(reader: &BinaryReader<&[u8]>) -> Result<()> {
    if reader.remaining != 0 {
        return Err(Error::manifest("trailing blueprint column bytes"));
    }
    Ok(())
}

pub(crate) fn decode(groups: &[Vec<u8>]) -> Result<Vec<SnapshotEntry>> {
    if groups.len() != 3 {
        return Err(Error::manifest("invalid blueprint groups"));
    }
    let mut paths = reader(&groups[0]);
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
    let mut core = reader(&groups[1]);
    core.require_records(entry_count as u64, 3)?;
    let kinds: Vec<_> = (0..entry_count)
        .map(|_| read_u8(&mut core))
        .collect::<Result<_>>()?;
    let flags: Vec<_> = (0..entry_count)
        .map(|_| read_u16(&mut core))
        .collect::<Result<_>>()?;
    for (&kind, &flag) in kinds.iter().zip(&flags) {
        let allowed = match kind {
            0 => 3,
            1 => 1,
            2 => 0,
            _ => return Err(Error::manifest("invalid entry kind")),
        };
        if flag & !allowed != 0 {
            return Err(Error::manifest("invalid entry flags"));
        }
    }
    let file_count = kinds.iter().filter(|&&kind| kind == 0).count();
    core.require_records(file_count as u64, 9)?;
    let sizes: Vec<_> = (0..file_count)
        .map(|_| read_u64(&mut core))
        .collect::<Result<_>>()?;
    let hashes: Vec<_> = sizes
        .iter()
        .map(|&size| {
            if size == 0 {
                Ok(ChunkHash::from_bytes(*blake3::hash(b"").as_bytes()))
            } else {
                read_chunk_hash(&mut core)
            }
        })
        .collect::<Result<_>>()?;
    let modes: Vec<_> = (0..file_count)
        .map(|_| read_u8(&mut core))
        .collect::<Result<_>>()?;
    let mut targets = Vec::new();
    for &kind in &kinds {
        if kind == 2 {
            let target = read_u32(&mut core)? as usize;
            if target >= entry_count {
                return Err(Error::manifest("invalid symlink target reference"));
            }
            targets.push(target);
        }
    }
    finished(&core)?;
    let mut chunks = reader(&groups[2]);
    let explicit_count = modes.iter().filter(|&&mode| mode == 2).count();
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
    finished(&chunks)?;
    let mut entries = Vec::with_capacity(entry_count);
    let (mut file_row, mut link_row, mut explicit_row) = (0, 0, 0);
    for (row, path) in entry_paths.iter().enumerate() {
        entries.push(match kinds[row] {
            0 => {
                let size = sizes[file_row];
                let hash = hashes[file_row].to_hex();
                let file_chunks = match modes[file_row] {
                    0 if size == 0 => Vec::new(),
                    1 if size > 0 && size <= crate::artifact::format::MAX_RAW_CHUNK_SIZE => {
                        vec![ChunkRef {
                            chunk_hash: hash.clone(),
                            raw_size: size as u32,
                        }]
                    }
                    2 if size > 0 => {
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
                    read_only: flags[row] & 1 != 0,
                    executable: flags[row] & 2 != 0,
                })
            }
            1 => SnapshotEntry::Directory(DirectoryManifest {
                path: path.clone(),
                read_only: flags[row] & 1 != 0,
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
        for (group, offset, value) in [
            (0, 8, 1),
            (0, 12, 2),
            (0, 20, 1),
            (1, 0, 9),
            (1, 1, 4),
            (1, 2, 1),
            (1, 43, 0),
            (2, 0, 1),
        ] {
            let mut invalid = valid.clone();
            invalid[group][offset] = value;
            assert!(decode(&invalid).is_err(), "group {group}, offset {offset}");
        }
        for group in 0..3 {
            for length in 0..valid[group].len() {
                let mut invalid = valid.clone();
                invalid[group].truncate(length);
                assert!(decode(&invalid).is_err(), "group {group}, length {length}");
            }
        }
    }

    #[test]
    fn native_corpus_layout_comparison() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repetitions = std::env::var("GUTCH_LAYOUT_REPETITIONS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(7)
            .clamp(1, 100);
        fn median_us(repetitions: usize, mut operation: impl FnMut()) -> u128 {
            let mut samples = Vec::new();
            for _ in 0..repetitions {
                let start = std::time::Instant::now();
                operation();
                samples.push(start.elapsed().as_micros());
            }
            samples.sort_unstable();
            samples[samples.len() / 2]
        }
        let mut report = Vec::new();
        let mut index_report = Vec::new();
        for item in walkdir::WalkDir::new(root.join("tests/datasets/corpus")) {
            let item = item.unwrap();
            if item.file_type().is_file() && item.file_name() == "chunk_map.gtcbin" {
                let bytes = std::fs::read(item.path()).unwrap();
                let index =
                    crate::repo::index::codec::load_chunk_index_binary_from_bytes(&bytes).unwrap();
                let decode_us = median_us(repetitions, || {
                    std::hint::black_box(
                        crate::repo::index::codec::load_chunk_index_binary_from_bytes(&bytes)
                            .unwrap(),
                    );
                });
                let lookup_us = median_us(repetitions, || {
                    for key in index.entries.keys() {
                        assert!(std::hint::black_box(index.entries.get_hash(key)).is_some());
                    }
                });
                let artifacts: std::collections::HashSet<_> = index
                    .entries
                    .values()
                    .map(|entry| std::sync::Arc::as_ptr(&entry.artifact))
                    .collect();
                index_report.push(serde_json::json!({"path": item.path().strip_prefix(root).unwrap(), "encoded_bytes": bytes.len(), "chunks": index.entries.len(), "shared_artifacts": artifacts.len(), "inline_bytes_per_chunk": std::mem::size_of::<(ChunkHash, manifest::ChunkIndexEntry)>(), "decode_median_us": decode_us, "all_key_lookups_median_us": lookup_us}));
                continue;
            }
            if !item.file_type().is_file() || item.file_name() != "snapshot_blueprint.gtcbin" {
                continue;
            }
            let original = std::fs::read(item.path()).unwrap();
            let entries =
                crate::manifest::binary::read_snapshot_blueprint_bytes(&original).unwrap();
            let columns = encode(&entries).unwrap();
            let magic = crate::artifact::format::SNAPSHOT_BLUEPRINT_MAGIC;
            let grouped = crate::artifact::metadata::encode(magic, &columns).unwrap();
            assert_eq!(
                grouped, original,
                "canonical writer must reproduce frozen metadata"
            );
            let mut merged = Vec::new();
            for column in &columns {
                write_u32(&mut merged, column.len() as u32).unwrap();
            }
            merged.extend_from_slice(&columns.concat());
            let whole = crate::artifact::metadata::encode(magic, &[merged]).unwrap();
            let decode_whole = || {
                let groups = crate::artifact::metadata::decode(&whole, magic, 1).unwrap();
                let mut merged = groups[0].as_slice();
                let lengths: Vec<_> = (0..3)
                    .map(|_| read_u32(&mut merged).unwrap() as usize)
                    .collect();
                let mut restored = Vec::new();
                for length in lengths {
                    let (column, rest) = merged.split_at(length);
                    restored.push(column.to_vec());
                    merged = rest;
                }
                assert!(merged.is_empty());
                decode(&restored).unwrap()
            };
            let restored =
                decode(&crate::artifact::metadata::decode(&grouped, magic, 3).unwrap()).unwrap();
            let mut sorted = entries.clone();
            sorted.sort_by(|a, b| a.path().cmp(b.path()));
            assert_eq!(
                serde_json::to_value(&restored).unwrap(),
                serde_json::to_value(&sorted).unwrap()
            );
            assert_eq!(
                serde_json::to_value(decode_whole()).unwrap(),
                serde_json::to_value(&sorted).unwrap()
            );
            let full_decode_us = median_us(repetitions, || {
                std::hint::black_box(
                    decode(&crate::artifact::metadata::decode(&grouped, magic, 3).unwrap())
                        .unwrap(),
                );
            });
            let whole_decode_us = median_us(repetitions, || {
                std::hint::black_box(decode_whole());
            });
            let columns_decode_us = median_us(repetitions, || {
                std::hint::black_box(decode(&columns).unwrap());
            });
            let encode_us = median_us(repetitions, || {
                std::hint::black_box(
                    crate::artifact::metadata::encode(magic, &encode(&entries).unwrap()).unwrap(),
                );
            });
            report.push(serde_json::json!({"path": item.path().strip_prefix(root).unwrap(), "current_bytes": original.len(), "whole_bytes": whole.len(), "group_bytes": grouped.len(), "full_decode_median_us": full_decode_us, "whole_decode_median_us": whole_decode_us, "column_validation_materialization_median_us": columns_decode_us, "encode_median_us": encode_us, "repetitions": repetitions}));
        }
        assert!(!report.is_empty(), "corpus blueprint files must be present");
        let directory = root.join("tests/.artifacts/binary-layouts");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("native.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        std::fs::write(
            directory.join("index-native.json"),
            serde_json::to_vec_pretty(&index_report).unwrap(),
        )
        .unwrap();
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    }
}
