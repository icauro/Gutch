use std::path::Path;

use crate::Error;
use crate::Result;
use crate::artifact::hash::hash_hex;
use crate::manifest::SnapshotEntry;

pub(crate) fn source_modified_at_from_directory(dir: &Path) -> Result<String> {
    Ok(format_system_time_utc(max_file_modified_time(dir)?))
}

pub(crate) fn format_system_time_utc(time: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
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
    // Capture already sorts entries. Avoid allocating or sorting again on that path.
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"gutch-installation-id-v2");
    hasher.update(&(entries.len() as u64).to_le_bytes());
    if entries
        .windows(2)
        .all(|pair| pair[0].path() <= pair[1].path())
    {
        for entry in entries {
            hash_installation_entry(&mut hasher, entry);
        }
    } else {
        let mut sorted_entries: Vec<_> = entries.iter().collect();
        sorted_entries.sort_unstable_by(|left, right| left.path().cmp(right.path()));
        for entry in sorted_entries {
            hash_installation_entry(&mut hasher, entry);
        }
    }
    hash_hex(&hasher.finalize())
}

fn hash_identity_field(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn hash_installation_entry(hasher: &mut blake3::Hasher, entry: &SnapshotEntry) {
    hash_identity_field(hasher, entry.path());
    match entry {
        SnapshotEntry::File(file) => {
            hasher.update(b"f");
            hash_identity_field(hasher, &file.full_file_hash);
            hasher.update(&[u8::from(file.executable), u8::from(file.read_only)]);
        }
        SnapshotEntry::Directory(directory) => {
            hasher.update(b"d");
            hasher.update(&[u8::from(directory.read_only)]);
        }
        SnapshotEntry::Symlink(link) => {
            hasher.update(b"l");
            hash_identity_field(hasher, &link.target_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::DirectoryManifest;
    use crate::manifest::SnapshotEntry;
    use crate::manifest::SymlinkManifest;

    #[test]
    fn identity_ignores_chunk_layout_and_preserves_installation_differences() {
        use crate::manifest::ChunkRef;
        use crate::manifest::FileManifest;
        let file = FileManifest {
            path: "bin/app".into(),
            size: 4,
            executable: true,
            read_only: false,
            full_file_hash: crate::artifact::hash::hash_hex(&blake3::hash(b"data")),
            chunks: vec![],
        };
        let directory = SnapshotEntry::Directory(DirectoryManifest {
            path: "empty".into(),
            read_only: false,
        });
        let id = compute_snapshot_id_from_entries(&[
            SnapshotEntry::File(file.clone()),
            directory.clone(),
        ]);
        let mut repacked = file.clone();
        repacked.chunks = vec![ChunkRef {
            chunk_hash: "a".repeat(64),
            raw_size: 4,
        }];
        assert_eq!(
            id,
            compute_snapshot_id_from_entries(&[directory.clone(), SnapshotEntry::File(repacked)])
        );
        for changed in [
            FileManifest {
                path: "bin/renamed".into(),
                ..file.clone()
            },
            FileManifest {
                executable: false,
                ..file.clone()
            },
            FileManifest {
                read_only: true,
                ..file.clone()
            },
            FileManifest {
                full_file_hash: crate::artifact::hash::hash_hex(&blake3::hash(b"edit")),
                ..file.clone()
            },
        ] {
            assert_ne!(
                id,
                compute_snapshot_id_from_entries(&[
                    SnapshotEntry::File(changed),
                    directory.clone()
                ])
            );
        }
        let link = |target: &str| {
            SnapshotEntry::Symlink(SymlinkManifest {
                path: "current".into(),
                target_path: target.into(),
            })
        };
        assert_ne!(
            compute_snapshot_id_from_entries(&[link("one")]),
            compute_snapshot_id_from_entries(&[link("two")])
        );
    }

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
