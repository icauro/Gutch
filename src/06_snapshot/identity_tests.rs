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
    let id = snapshot_id(
        &[SnapshotEntry::File(file.clone()), directory.clone()],
        None,
    );
    let mut repacked = file.clone();
    repacked.chunks = vec![ChunkRef {
        chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&"a".repeat(64)).unwrap(),
        raw_size: 4,
    }];
    assert_eq!(
        id,
        snapshot_id(&[directory.clone(), SnapshotEntry::File(repacked)], None)
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
            snapshot_id(&[SnapshotEntry::File(changed), directory.clone()], None)
        );
    }
    let link = |target: &str| {
        SnapshotEntry::Symlink(SymlinkManifest {
            path: "current".into(),
            target_path: target.into(),
        })
    };
    assert_ne!(
        snapshot_id(&[link("one")], None),
        snapshot_id(&[link("two")], None)
    );
    let read_only_directory = SnapshotEntry::Directory(DirectoryManifest {
        path: "empty".into(),
        read_only: true,
    });
    assert_ne!(
        id,
        snapshot_id(
            &[SnapshotEntry::File(file.clone()), read_only_directory],
            None
        )
    );
    let directory_at_file_path = SnapshotEntry::Directory(DirectoryManifest {
        path: file.path.clone(),
        read_only: false,
    });
    assert_ne!(
        snapshot_id(&[SnapshotEntry::File(file)], None),
        snapshot_id(&[directory_at_file_path], None)
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
        snapshot_id(&[], None),
        "7fc0260419cc769d8b36885f26217943238a988ca6aa722179645f5548486a94"
    );
    assert_eq!(
        snapshot_id(&entries, None),
        "8368fd370c0898f5c1aacc27c9d0bce45071d66be95bd7f6df8fd57400fc7a8a"
    );
}
