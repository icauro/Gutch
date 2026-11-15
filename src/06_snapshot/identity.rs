//! Content-derived snapshot identity.

use crate::artifact::hash::hash_hex;
use crate::manifest::SnapshotEntry;

/// Hashes entries in path order, so the ID ignores input order. Paths must be
/// unique, as manifest validation guarantees; capture already sorts entries,
/// so that path neither allocates nor sorts again.
pub(crate) fn snapshot_id_from_entries(entries: &[SnapshotEntry]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"gutch-installation-id-v2");
    hasher.update(&(entries.len() as u64).to_le_bytes());
    if entries
        .windows(2)
        .all(|pair| pair[0].path() <= pair[1].path())
    {
        for entry in entries {
            hash_identity_entry(&mut hasher, entry);
        }
    } else {
        let mut sorted_entries: Vec<_> = entries.iter().collect();
        sorted_entries.sort_unstable_by(|left, right| left.path().cmp(right.path()));
        for entry in sorted_entries {
            hash_identity_entry(&mut hasher, entry);
        }
    }
    hash_hex(&hasher.finalize())
}

fn hash_identity_field(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn hash_identity_entry(hasher: &mut blake3::Hasher, entry: &SnapshotEntry) {
    hash_identity_field(hasher, entry.path());
    match entry {
        SnapshotEntry::File(file) => {
            hasher.update(b"f");
            hash_identity_field(hasher, &file.full_file_hash.to_hex());
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
            full_file_hash: crate::artifact::hash::ChunkHash::of(b"data"),
            chunks: vec![],
        };
        let directory = SnapshotEntry::Directory(DirectoryManifest {
            path: "empty".into(),
            read_only: false,
        });
        let id = snapshot_id_from_entries(&[SnapshotEntry::File(file.clone()), directory.clone()]);
        let mut repacked = file.clone();
        repacked.chunks = vec![ChunkRef {
            chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&"a".repeat(64)).unwrap(),
            raw_size: 4,
        }];
        assert_eq!(
            id,
            snapshot_id_from_entries(&[directory.clone(), SnapshotEntry::File(repacked)])
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
                full_file_hash: crate::artifact::hash::ChunkHash::of(b"edit"),
                ..file.clone()
            },
        ] {
            assert_ne!(
                id,
                snapshot_id_from_entries(&[SnapshotEntry::File(changed), directory.clone()])
            );
        }
        let link = |target: &str| {
            SnapshotEntry::Symlink(SymlinkManifest {
                path: "current".into(),
                target_path: target.into(),
            })
        };
        assert_ne!(
            snapshot_id_from_entries(&[link("one")]),
            snapshot_id_from_entries(&[link("two")])
        );
        let read_only_directory = SnapshotEntry::Directory(DirectoryManifest {
            path: "empty".into(),
            read_only: true,
        });
        assert_ne!(
            id,
            snapshot_id_from_entries(&[SnapshotEntry::File(file.clone()), read_only_directory])
        );
        let directory_at_file_path = SnapshotEntry::Directory(DirectoryManifest {
            path: file.path.clone(),
            read_only: false,
        });
        assert_ne!(
            snapshot_id_from_entries(&[SnapshotEntry::File(file)]),
            snapshot_id_from_entries(&[directory_at_file_path])
        );
    }

    /// Stored repositories depend on this exact encoding; a change here makes
    /// every existing snapshot fail its integrity check.
    #[test]
    fn identity_encoding_is_pinned() {
        let entries = [
            SnapshotEntry::File(crate::manifest::FileManifest {
                path: "bin/app".into(),
                size: 4,
                executable: true,
                read_only: false,
                full_file_hash: crate::artifact::hash::ChunkHash::of(b"data"),
                chunks: vec![],
            }),
            SnapshotEntry::Symlink(SymlinkManifest {
                path: "current".into(),
                target_path: "bin".into(),
            }),
            SnapshotEntry::Directory(DirectoryManifest {
                path: "empty".into(),
                read_only: true,
            }),
        ];
        assert_eq!(
            snapshot_id_from_entries(&[]),
            "7fc0260419cc769d8b36885f26217943238a988ca6aa722179645f5548486a94"
        );
        assert_eq!(
            snapshot_id_from_entries(&entries),
            "8368fd370c0898f5c1aacc27c9d0bce45071d66be95bd7f6df8fd57400fc7a8a"
        );
    }
}
