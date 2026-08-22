use serde::{Deserialize, Serialize};

use super::ArtifactStorageMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    pub path: String,
    pub size: u64,
    pub executable: bool,
    pub read_only: bool,
    pub full_file_hash: String,
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

pub fn validate_snapshot_entries(entries: &[SnapshotEntry]) -> crate::Result<()> {
    let mut paths = std::collections::HashSet::with_capacity(entries.len());
    for entry in entries {
        crate::validate_manifest_relative_path(entry.path(), false)?;
        if !paths.insert(entry.path()) {
            return Err(crate::Error::manifest(format!(
                "snapshot blueprint contains duplicate path '{}'",
                entry.path()
            )));
        }
        if let SnapshotEntry::File(file) = entry {
            let chunk_bytes = file.chunks.iter().try_fold(0u64, |total, chunk| {
                total
                    .checked_add(chunk.raw_size)
                    .ok_or_else(|| crate::Error::manifest("snapshot blueprint chunk size overflow"))
            })?;
            if !file.chunks.is_empty() && chunk_bytes != file.size {
                return Err(crate::Error::manifest(format!(
                    "snapshot blueprint file '{}' size does not match its chunks",
                    file.path
                )));
            }
        }
    }
    for entry in entries {
        if let SnapshotEntry::Symlink(link) = entry {
            crate::validate_manifest_relative_path(&link.target_path, false)?;
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
    pub chunk_hash: String,
    pub raw_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleTableEntry {
    pub chunk_hash: String,
    pub raw_size: u64,
    pub compressed_size: u64,
    pub compressed_offset: u64,
    #[serde(default)]
    pub raw_offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Estimate {
    pub missing_logical_bytes: u64,
    pub missing_compressed_bytes: u64,
    pub missing_chunk_count: u64,
    pub required_bundle_count: u64,
    pub range_request_count: u64,
    pub local_cache_hits: u64,
    pub available_disk_space: u64,
    pub fragmentation_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkIndex {
    pub format_version: String,
    pub updated_at: String,
    pub entries: std::collections::BTreeMap<String, ChunkIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChunkIndexEntry {
    pub bundle_id: String,
    pub raw_size: u64,
    pub compressed_size: u64,
    #[serde(default)]
    pub storage_mode: ArtifactStorageMode,
}
