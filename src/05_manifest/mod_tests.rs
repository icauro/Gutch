use super::*;

#[test]
fn test_file_manifest_serialization() {
    let fm = FileManifest {
        path: "game.exe".to_string(),
        size: 12345,
        executable: true,
        read_only: false,
        full_file_hash: "01".repeat(32),
        chunks: vec![
            ChunkRef {
                chunk_hash: crate::artifact::hash::ChunkHash::from_bytes([2; 32]),
                raw_size: 8192,
            },
            ChunkRef {
                chunk_hash: crate::artifact::hash::ChunkHash::from_bytes([3; 32]),
                raw_size: 4153,
            },
        ],
    };
    let json = serde_json::to_string(&fm).unwrap();
    let fm2: FileManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(fm.path, fm2.path);
    assert_eq!(fm.chunks.len(), 2);
}

#[test]
fn test_structured_version_parses_three_parts() {
    let version = StructuredVersion::parse("v1.2.3").unwrap();
    assert_eq!(version.major, 1);
    assert_eq!(version.minor, 2);
    assert_eq!(version.patch, 3);
    assert_eq!(version.revision, 0);
    assert_eq!(version.canonical_string(), "1.2.3");
}

#[test]
fn test_structured_version_parses_revision() {
    let version = StructuredVersion::parse("2025.05.30.1").unwrap();
    assert_eq!(version.major, 2025);
    assert_eq!(version.minor, 5);
    assert_eq!(version.patch, 30);
    assert_eq!(version.revision, 1);
    assert_eq!(version.canonical_string(), "2025.5.30.1");
}

#[test]
fn test_structured_version_accepts_generic_revision_and_omits_zero() {
    assert_eq!(
        StructuredVersion::parse("1.2.3.4")
            .unwrap()
            .canonical_string(),
        "1.2.3.4"
    );
    assert_eq!(
        StructuredVersion::parse("1.2.3.0")
            .unwrap()
            .canonical_string(),
        "1.2.3"
    );
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
