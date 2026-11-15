//! Snapshot entries: files with their chunk references, directories and symlinks.

use crate::artifact::format::MAX_RAW_CHUNK_SIZE;
use crate::artifact::hash::ChunkHash;
use crate::filesystem::paths::validate_manifest_relative_path;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    pub path: String,
    pub size: u64,
    pub executable: bool,
    pub read_only: bool,
    pub full_file_hash: ChunkHash,
    pub chunks: Vec<ChunkRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryManifest {
    pub path: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymlinkManifest {
    pub path: String,
    pub target_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "entry_type", rename_all = "snake_case")]
pub enum SnapshotEntry {
    File(FileManifest),
    Directory(DirectoryManifest),
    Symlink(SymlinkManifest),
}

impl From<FileManifest> for SnapshotEntry {
    fn from(value: FileManifest) -> Self {
        Self::File(value)
    }
}

impl SnapshotEntry {
    pub fn path(&self) -> &str {
        match self {
            Self::File(entry) => &entry.path,
            Self::Directory(entry) => &entry.path,
            Self::Symlink(entry) => &entry.path,
        }
    }

    pub fn as_file(&self) -> Option<&FileManifest> {
        match self {
            Self::File(entry) => Some(entry),
            _ => None,
        }
    }
}

pub(crate) fn validate_snapshot_entries(entries: &[SnapshotEntry]) -> crate::Result<()> {
    let mut paths = HashSet::with_capacity(entries.len());
    for entry in entries {
        validate_manifest_relative_path(entry.path(), false)?;
        if !paths.insert(entry.path()) {
            return Err(crate::Error::manifest(format!(
                "snapshot blueprint contains duplicate path '{}'",
                entry.path()
            )));
        }
        if let SnapshotEntry::File(file) = entry {
            for chunk in &file.chunks {
                if chunk.raw_size == 0 || u64::from(chunk.raw_size) > MAX_RAW_CHUNK_SIZE {
                    return Err(crate::Error::manifest("invalid snapshot chunk size"));
                }
            }
            let chunk_bytes = file.chunks.iter().try_fold(0u64, |total, chunk| {
                total
                    .checked_add(u64::from(chunk.raw_size))
                    .ok_or_else(|| crate::Error::manifest("snapshot blueprint chunk size overflow"))
            })?;
            if chunk_bytes != file.size {
                return Err(crate::Error::manifest(format!(
                    "snapshot blueprint file '{}' size does not match its chunks",
                    file.path
                )));
            }
        }
    }
    let non_directories: HashSet<_> = entries
        .iter()
        .filter(|entry| !matches!(entry, SnapshotEntry::Directory(_)))
        .map(SnapshotEntry::path)
        .collect();
    for entry in entries {
        for (offset, _) in entry.path().match_indices('/') {
            if non_directories.contains(&entry.path()[..offset]) {
                return Err(crate::Error::manifest(
                    "snapshot path has a non-directory ancestor",
                ));
            }
        }
        if let SnapshotEntry::Symlink(link) = entry {
            validate_manifest_relative_path(&link.target_path, false)?;
            if !paths.contains(link.target_path.as_str()) {
                return Err(crate::Error::manifest(format!(
                    "snapshot symlink '{}' has missing target '{}'",
                    link.path, link.target_path
                )));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRef {
    pub chunk_hash: ChunkHash,
    pub raw_size: u32,
}

/// Measured artifact sizes for a fresh capture with the default profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Estimate {
    pub unique_chunk_bytes: u64,
    pub unique_chunk_count: usize,
    pub encoded_artifact_bytes: u64,
    pub artifact_count: usize,
    pub bundle_count: usize,
}
