use super::*;
use crate::artifact::hash::ChunkHash;
use crate::manifest::{ChunkRef, DirectoryManifest, FileManifest, SymlinkManifest};

fn file(path: &str, size: u64) -> SnapshotEntry {
    FileManifest {
        path: path.into(),
        size,
        executable: false,
        read_only: false,
        full_file_hash: ChunkHash::from_bytes([0xaa; 32]),
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
        Some(SnapshotChangeKind::PropertiesChanged)
    );
    repacked.full_file_hash = ChunkHash::from_bytes([0xcc; 32]);
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
        Some(SnapshotChangeKind::PropertiesChanged)
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
    assert_eq!(changed.changes[0].change, SnapshotChangeKind::TypeChanged);
    assert_eq!(changed.delta_bytes, -4);
}

#[test]
fn diff_lists_changes_in_path_order_and_omits_unchanged_entries() {
    let before = [file("z", 1), file("b", 2), file("same", 3), file("a", 1)];
    let after = [file("same", 3), file("c", 5), file("a", 4), file("z", 1)];
    let diff = compare_snapshot_entries(&before, &after, &Default::default()).unwrap();
    let changes = diff
        .changes
        .iter()
        .map(|change| (change.path.as_str(), change.change, change.delta_bytes))
        .collect::<Vec<_>>();
    assert_eq!(
        changes,
        [
            ("a", SnapshotChangeKind::Modified, 3),
            ("b", SnapshotChangeKind::Removed, -2),
            ("c", SnapshotChangeKind::Added, 5),
        ]
    );
    assert_eq!(diff.delta_bytes, 6);
    assert!(
        compare_snapshot_entries(&after, &after, &Default::default())
            .unwrap()
            .changes
            .is_empty()
    );
}

#[test]
fn change_kind_labels_match_serialized_names() {
    use SnapshotChangeKind::*;
    for kind in [Added, Removed, Modified, PropertiesChanged, TypeChanged] {
        assert_eq!(
            serde_json::to_string(&kind).unwrap(),
            format!("\"{}\"", kind.as_str())
        );
    }
}
