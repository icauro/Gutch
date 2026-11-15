use super::*;

fn read_snapshot_blueprint_binary(path: &Path) -> Result<Vec<manifest::SnapshotEntry>> {
    read_snapshot_blueprint_bytes(&metadata::read_file(path)?)
}
use crate::artifact::hash::ChunkHash;

#[test]
fn rejects_impossible_blueprint_column_counts_before_allocation() {
    let blueprint = metadata::encode(
        SNAPSHOT_BLUEPRINT_MAGIC,
        &[
            [u32::MAX.to_le_bytes(), u32::MAX.to_le_bytes()].concat(),
            Vec::new(),
            vec![0; 4],
        ],
    )
    .unwrap();
    assert!(read_snapshot_blueprint_bytes(&blueprint).is_err());
}

#[test]
fn blueprint_preserves_file_sizes_above_u32_without_wide_chunk_sizes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("blueprint");
    let size = 1u64 << 32;
    let blueprint = SnapshotBlueprint {
        entries: vec![
            manifest::FileManifest {
                path: "large.bin".into(),
                size,
                executable: false,
                read_only: false,
                full_file_hash: ChunkHash::from_bytes([1; 32]),
                chunks: vec![
                    manifest::ChunkRef {
                        chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&"02".repeat(32))
                            .unwrap(),
                        raw_size: 4 * 1024 * 1024
                    };
                    1024
                ],
            }
            .into(),
        ],
    };
    write_snapshot_blueprint_binary(&path, &blueprint).unwrap();
    let restored = read_snapshot_blueprint_binary(&path).unwrap();
    assert_eq!(restored[0].as_file().unwrap().size, size);
    assert!(std::fs::metadata(path).unwrap().len() < 32 + 51 + 9 + 36 * 1024);
}

fn all_entries() -> Vec<manifest::SnapshotEntry> {
    vec![
        manifest::SnapshotEntry::Directory(manifest::DirectoryManifest {
            path: "empty".into(),
            read_only: false,
        }),
        manifest::SnapshotEntry::File(manifest::FileManifest {
            path: "game.bin".into(),
            size: 0,
            executable: true,
            read_only: true,
            full_file_hash: ChunkHash::of(b""),
            chunks: Vec::new(),
        }),
        manifest::SnapshotEntry::Symlink(manifest::SymlinkManifest {
            path: "current".into(),
            target_path: "empty".into(),
        }),
    ]
}

#[test]
fn version_one_blueprint_round_trips_all_entry_kinds() {
    let path_temp = tempfile::tempdir().unwrap();
    let path = path_temp.path().join("entry_blueprint_roundtrip");
    let blueprint = SnapshotBlueprint {
        entries: all_entries(),
    };
    write_snapshot_blueprint_binary(&path, &blueprint).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(u16::from_le_bytes([bytes[16], bytes[17]]), 1);
    let loaded = read_snapshot_blueprint_binary(&path).unwrap();
    assert_eq!(loaded.len(), 3);
    assert!(matches!(loaded[1], manifest::SnapshotEntry::Directory(_)));
    assert!(matches!(loaded[2], manifest::SnapshotEntry::File(_)));
    assert!(matches!(loaded[0], manifest::SnapshotEntry::Symlink(_)));
    let _ = std::fs::remove_file(path);
}

#[test]
fn blueprint_rejects_unknown_entry_kind() {
    let path_temp = tempfile::tempdir().unwrap();
    let path = path_temp.path().join("entry_blueprint_bad_kind");
    write_snapshot_blueprint_binary(
        &path,
        &SnapshotBlueprint {
            entries: all_entries(),
        },
    )
    .unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[32] = 9;
    std::fs::write(&path, bytes).unwrap();
    assert!(read_snapshot_blueprint_binary(&path).is_err());
    let _ = std::fs::remove_file(path);
}
