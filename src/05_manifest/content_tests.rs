use super::*;
use crate::artifact::hash::ChunkHash;

#[test]
fn file_manifest_serialization() {
    let fm = FileManifest {
        path: "game.exe".to_string(),
        size: 12345,
        executable: true,
        read_only: false,
        full_file_hash: ChunkHash::from_bytes([1; 32]),
        chunks: vec![
            ChunkRef {
                chunk_hash: ChunkHash::from_bytes([2; 32]),
                raw_size: 8192,
            },
            ChunkRef {
                chunk_hash: ChunkHash::from_bytes([3; 32]),
                raw_size: 4153,
            },
        ],
    };
    let json = serde_json::to_string(&fm).unwrap();
    let fm2: FileManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(fm, fm2);
}

fn file(path: &str, bytes: &[u8]) -> SnapshotEntry {
    FileManifest {
        path: path.into(),
        size: bytes.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: ChunkHash::of(bytes),
        chunks: (!bytes.is_empty())
            .then(|| ChunkRef {
                chunk_hash: ChunkHash::of(bytes),
                raw_size: bytes.len() as u32,
            })
            .into_iter()
            .collect(),
    }
    .into()
}

#[test]
fn entry_validation_reports_damaged_data_as_manifest_errors() {
    assert!(validate_snapshot_entries(&[file("a", b""), file("b", b"data")]).is_ok());

    let mut empty_with_content_hash = file("a", b"");
    if let SnapshotEntry::File(file) = &mut empty_with_content_hash {
        file.full_file_hash = ChunkHash::of(b"data");
    }
    for entries in [
        vec![empty_with_content_hash],
        vec![file("../escape", b"data")],
        vec![file("a\\b", b"data")],
    ] {
        let error = validate_snapshot_entries(&entries).unwrap_err();
        assert!(matches!(error, crate::Error::Manifest(_)), "{error:?}");
    }
}

#[test]
fn chunk_reference_json_preserves_canonical_hex_and_rejects_invalid_hashes() {
    let expected = serde_json::json!({"chunk_hash": "ab".repeat(32), "raw_size": 123});
    let chunk: ChunkRef = serde_json::from_value(expected.clone()).unwrap();
    assert_eq!(chunk.chunk_hash.as_bytes(), &[0xab; 32]);
    assert_eq!(chunk.raw_size, 123);
    assert_eq!(serde_json::to_value(&chunk).unwrap(), expected);
    for invalid in [
        String::new(),
        "a".repeat(63),
        "a".repeat(65),
        "AB".repeat(32),
        "gg".repeat(32),
        "é".repeat(32),
    ] {
        assert!(
            serde_json::from_value::<ChunkRef>(serde_json::json!({
                "chunk_hash": invalid, "raw_size": 123,
            }))
            .is_err()
        );
    }
}
