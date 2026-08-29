use std::path::Path;

use crate::hash::hash_hex;
use crate::manifest::SnapshotEntry;
use crate::{Error, Result};

pub(crate) fn source_modified_at_from_directory(dir: &Path) -> Result<String> {
    Ok(format_system_time_utc(max_file_modified_time(dir)?))
}

pub(crate) fn format_system_time_utc(time: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

pub(crate) fn is_source_modified_at_newer(candidate: &str, recorded: &str) -> bool {
    if recorded.trim().is_empty() {
        return true;
    }
    let Some(recorded_time) = parse_source_modified_at(recorded) else {
        return true;
    };
    let Some(candidate_time) = parse_source_modified_at(candidate) else {
        return false;
    };
    candidate_time > recorded_time
}

fn parse_source_modified_at(value: &str) -> Option<std::time::SystemTime> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(std::time::SystemTime::from)
}

fn max_file_modified_time(dir: &Path) -> Result<std::time::SystemTime> {
    if !dir.is_dir() {
        return Err(Error::manifest(format!(
            "source path is not a directory: {}",
            dir.display()
        )));
    }

    let mut max_mtime = std::time::SystemTime::UNIX_EPOCH;
    let mut found_file = false;
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let mtime = std::fs::symlink_metadata(entry.path())?.modified()?;
        if !found_file || mtime > max_mtime {
            max_mtime = mtime;
            found_file = true;
        }
    }

    if found_file {
        return Ok(max_mtime);
    }

    dir.metadata()?.modified().map_err(|err| {
        Error::manifest(format!(
            "failed to read source directory mtime for {}: {err}",
            dir.display()
        ))
    })
}

pub(crate) fn compute_snapshot_id_from_entries(entries: &[SnapshotEntry]) -> String {
    let mut sorted_entries = entries.iter().collect::<Vec<_>>();
    sorted_entries.sort_by(|left, right| left.path().cmp(right.path()));

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"gutch-snapshot-entry-id-v1");
    for entry in sorted_entries {
        hasher.update(entry.path().as_bytes());
        hasher.update(b"\x00");
        match entry {
            SnapshotEntry::File(file) => {
                hasher.update(b"file\x00");
                hasher.update(&file.size.to_le_bytes());
                hasher.update(&[u8::from(file.executable), u8::from(file.read_only)]);
                hasher.update(file.full_file_hash.as_bytes());
                for chunk in &file.chunks {
                    hasher.update(chunk.chunk_hash.as_bytes());
                    hasher.update(&chunk.raw_size.to_le_bytes());
                }
            }
            SnapshotEntry::Directory(directory) => {
                hasher.update(b"directory\x00");
                hasher.update(&[u8::from(directory.read_only)]);
            }
            SnapshotEntry::Symlink(link) => {
                hasher.update(b"symlink\x00");
                hasher.update(link.target_path.as_bytes());
            }
        }
        hasher.update(b"\x00");
    }
    hash_hex(&hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{DirectoryManifest, SnapshotEntry, SymlinkManifest};

    #[test]
    fn identity_changes_with_supported_filesystem_metadata() {
        let directory = SnapshotEntry::Directory(DirectoryManifest {
            path: "target".into(),
            read_only: false,
        });
        let link = SnapshotEntry::Symlink(SymlinkManifest {
            path: "current".into(),
            target_path: "target".into(),
        });
        let original = compute_snapshot_id_from_entries(&[directory.clone(), link.clone()]);
        let changed_permissions = SnapshotEntry::Directory(DirectoryManifest {
            path: "target".into(),
            read_only: true,
        });
        assert_ne!(
            original,
            compute_snapshot_id_from_entries(&[changed_permissions, link])
        );
    }
}
