use super::*;

#[test]
fn rejects_impossible_index_column_counts_before_allocation() {
    let index = metadata::encode(
        CHUNK_INDEX_BINARY_MAGIC,
        &[vec![0; 4], u32::MAX.to_le_bytes().to_vec()],
    )
    .unwrap();
    assert!(load_chunk_index_binary_from_bytes(&index).is_err());
}

#[test]
fn independent_index_fixture_rejects_damaged_framing_and_trailing_bytes() {
    let mut artifacts = 1u32.to_le_bytes().to_vec();
    artifacts.extend_from_slice(&[7; 32]);
    artifacts.extend_from_slice(&200u32.to_le_bytes());
    let mut chunks = 1u32.to_le_bytes().to_vec();
    chunks.extend_from_slice(&[9; 32]);
    chunks.extend_from_slice(&0u32.to_le_bytes());
    chunks.extend_from_slice(&0u32.to_le_bytes());
    chunks.extend_from_slice(&128u32.to_le_bytes());
    let mut bytes = metadata::encode(CHUNK_INDEX_BINARY_MAGIC, &[artifacts, chunks]).unwrap();
    let index = load_chunk_index_binary_from_bytes(&bytes).unwrap();
    let entry = index.entries.get(&ChunkHash::from_bytes([9; 32])).unwrap();
    assert_eq!(entry.bundle_id.as_str(), "07".repeat(32));
    assert_eq!(entry.artifact_size, 200);
    assert_eq!(entry.raw_size, 128);
    for offset in [0, 15, 16, 18, 72, 108] {
        let mut malformed = bytes.clone();
        malformed[offset] = 255;
        assert!(
            load_chunk_index_binary_from_bytes(&malformed).is_err(),
            "offset {offset}"
        );
    }
    bytes.push(0);
    assert!(load_chunk_index_binary_from_bytes(&bytes).is_err());
}

#[test]
fn mixed_storage_roundtrip_is_deterministic_and_shares_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index");
    let bundle = smol_str::SmolStr::new("07".repeat(32));
    let loose = ChunkHash::from_bytes([3; 32]);
    let mut index = crate::repo::index::ChunkIndex {
        entries: [
            (
                ChunkHash::from_bytes([1; 32]),
                crate::repo::index::ChunkIndexEntry::new(
                    100,
                    0,
                    bundle.clone(),
                    10,
                    crate::artifact::profiles::ArtifactStorageMode::Bundled,
                ),
            ),
            (
                ChunkHash::from_bytes([2; 32]),
                crate::repo::index::ChunkIndexEntry::new(
                    100,
                    1,
                    bundle,
                    20,
                    crate::artifact::profiles::ArtifactStorageMode::Bundled,
                ),
            ),
            (
                loose,
                crate::repo::index::ChunkIndexEntry::new(
                    60,
                    0,
                    loose.to_hex().into(),
                    4,
                    crate::artifact::profiles::ArtifactStorageMode::LooseChunks,
                ),
            ),
        ]
        .into_iter()
        .collect(),
    };
    write_chunk_index_binary(&path, &index).unwrap();
    let first = std::fs::read(&path).unwrap();
    let groups = crate::artifact::metadata::decode(&first, CHUNK_INDEX_BINARY_MAGIC, 2).unwrap();
    for group in 0..groups.len() {
        for length in 0..groups[group].len() {
            let mut truncated = groups.clone();
            truncated[group].truncate(length);
            assert!(
                read_chunk_index_binary(&truncated).is_err(),
                "group {group}, length {length}"
            );
        }
    }
    let mut trailing = groups.clone();
    trailing[1].push(0);
    let error = read_chunk_index_binary(&trailing).unwrap_err();
    assert!(error.to_string().contains("trailing bytes"));
    for (group, offset, value) in [
        (0, 36, 0u32),
        (1, 100, u32::MAX - 1),
        (1, 116, 0),
        (1, 124, 0),
        (
            1,
            124,
            crate::artifact::format::MAX_RAW_CHUNK_SIZE as u32 + 1,
        ),
    ] {
        let mut invalid = groups.clone();
        invalid[group][offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        let bytes = crate::artifact::metadata::encode(CHUNK_INDEX_BINARY_MAGIC, &invalid).unwrap();
        assert!(
            load_chunk_index_binary_from_bytes(&bytes).is_err(),
            "group {group}, offset {offset}"
        );
    }
    let mut duplicate = groups.clone();
    let first_hash = duplicate[1][4..36].to_vec();
    duplicate[1][36..68].copy_from_slice(&first_hash);
    assert!(
        load_chunk_index_binary_from_bytes(
            &crate::artifact::metadata::encode(CHUNK_INDEX_BINARY_MAGIC, &duplicate).unwrap()
        )
        .is_err()
    );
    let mut unsorted = groups.clone();
    let first_hash = unsorted[1][4..36].to_vec();
    let second_hash = unsorted[1][36..68].to_vec();
    unsorted[1][4..36].copy_from_slice(&second_hash);
    unsorted[1][36..68].copy_from_slice(&first_hash);
    let error = load_chunk_index_binary_from_bytes(
        &crate::artifact::metadata::encode(CHUNK_INDEX_BINARY_MAGIC, &unsorted).unwrap(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unique and sorted"));
    let restored = load_chunk_index_binary(&path).unwrap();
    assert_eq!(restored.entries.len(), 3);
    let entries: Vec<_> = restored.entries.values().collect();
    assert!(std::sync::Arc::ptr_eq(
        &entries[0].artifact,
        &entries[1].artifact
    ));
    assert_eq!(
        entries[2].storage_mode,
        crate::artifact::profiles::ArtifactStorageMode::LooseChunks
    );
    write_chunk_index_binary(&path, &restored).unwrap();
    assert_eq!(first, std::fs::read(&path).unwrap());
    index.entries.values_mut().last().unwrap().chunk_ordinal = 1;
    assert!(write_chunk_index_binary(&path, &index).is_err());
    assert_eq!(first, std::fs::read(&path).unwrap());
}

#[test]
fn writer_rejects_conflicting_sizes_and_noncanonical_artifact_ids_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index");
    let index = crate::repo::index::ChunkIndex {
        entries: (0..2u8)
            .map(|ordinal| {
                (
                    ChunkHash::from_bytes([ordinal; 32]),
                    crate::repo::index::ChunkIndexEntry::new(
                        128,
                        u32::from(ordinal),
                        "ab".repeat(32).into(),
                        16,
                        crate::artifact::profiles::ArtifactStorageMode::Bundled,
                    ),
                )
            })
            .collect(),
    };
    write_chunk_index_binary(&path, &index).unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut inconsistent = index.clone();
    let last = inconsistent.entries.values_mut().last().unwrap();
    std::sync::Arc::make_mut(&mut last.artifact).artifact_size += 1;
    assert!(write_chunk_index_binary(&path, &inconsistent).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);

    for invalid_id in ["AB".repeat(32), "xx".repeat(32), "ab".repeat(31)] {
        let mut invalid = index.clone();
        let last = invalid.entries.values_mut().last().unwrap();
        std::sync::Arc::make_mut(&mut last.artifact).bundle_id = invalid_id.into();
        assert!(write_chunk_index_binary(&path, &invalid).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}
