use super::*;
use crate::artifact::hash::ChunkHash;
use crate::manifest::{ChunkRef, DirectoryManifest, FileManifest, SymlinkManifest};

fn file(path: &str, size: u64) -> SnapshotEntry {
    FileManifest {
        path: path.into(),
        size,
        executable: false,
        read_only: false,
        full_file_hash: "a".repeat(64),
        chunks: vec![],
    }
    .into()
}

#[test]
fn diff_preserves_full_unsigned_file_sizes_as_signed_deltas() {
    let entries = [file("one", u64::MAX), file("two", u64::MAX)];
    let added = compare_snapshot_entries(&[], &entries, &Default::default()).unwrap();
    assert_eq!(added.delta_bytes, i128::from(u64::MAX) * 2);
    assert_eq!(added.changes[0].delta_bytes, i128::from(u64::MAX));
    let removed = compare_snapshot_entries(&entries, &[], &Default::default()).unwrap();
    assert_eq!(removed.delta_bytes, -added.delta_bytes);
    assert!(
        serde_json::to_string(&removed)
            .unwrap()
            .contains("-36893488147419103230")
    );
}

#[test]
fn diff_ignores_packaging_but_preserves_properties_links_and_type_changes() {
    let base = file("file", 4);
    let SnapshotEntry::File(mut repacked) = base.clone() else {
        unreachable!()
    };
    repacked.chunks = vec![ChunkRef {
        chunk_hash: ChunkHash::parse_hex(&"b".repeat(64)).unwrap(),
        raw_size: 4,
    }];
    assert_eq!(changed_entry_kind(&base, &repacked.clone().into()), None);
    repacked.executable = true;
    assert_eq!(
        changed_entry_kind(&base, &repacked.clone().into()),
        Some(SnapshotChangeKind::Metadata)
    );
    repacked.full_file_hash = "c".repeat(64);
    assert_eq!(
        changed_entry_kind(&base, &repacked.into()),
        Some(SnapshotChangeKind::Modified)
    );
    let directory = |read_only| {
        SnapshotEntry::Directory(DirectoryManifest {
            path: "file".into(),
            read_only,
        })
    };
    assert_eq!(
        changed_entry_kind(&directory(false), &directory(true)),
        Some(SnapshotChangeKind::Metadata)
    );
    let link = |target: &str| {
        SnapshotEntry::Symlink(SymlinkManifest {
            path: "file".into(),
            target_path: target.into(),
        })
    };
    assert_eq!(
        changed_entry_kind(&link("one"), &link("two")),
        Some(SnapshotChangeKind::Modified)
    );
    assert_eq!(changed_entry_kind(&link("one"), &link("one")), None);
    let changed =
        compare_snapshot_entries(&[base], &[directory(false)], &Default::default()).unwrap();
    assert_eq!(changed.changes[0].change, SnapshotChangeKind::Modified);
    assert_eq!(changed.delta_bytes, -4);
}
