use super::*;

#[test]
fn bundle_sizing_policy_zero_bytes_uses_preferred() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(0), 10);
}

#[test]
fn bundle_sizing_policy_below_preferred_uses_preferred() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(5), 10);
}

#[test]
fn bundle_sizing_policy_prefers_preferred_for_large_inputs() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(1_000), 10);
}

#[test]
fn bundle_sizing_policy_slightly_above_preferred_splits_evenly() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(15), 10);
}

#[test]
fn bundle_sizing_policy_exactly_preferred() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(10), 10);
}

#[test]
fn bundle_sizing_policy_caps_invalid_preferred_above_max() {
    let policy = BundleSizingPolicy::new(100, 200);
    assert_eq!(policy.snapshot_size_for(1_000), 100);
}

#[test]
fn bundle_sizing_policy_exact_preferred_divisor() {
    let policy = BundleSizingPolicy::new(100, 10);
    assert_eq!(policy.snapshot_size_for(200), 10);
}

#[test]
fn bundle_sizing_policy_default_sizes() {
    let policy = BundleSizingPolicy::default();
    assert!(policy.max_size > 0);
    assert!(policy.preferred_size > 0);
    assert!(policy.max_size >= policy.preferred_size);
}

#[test]
fn bundle_raw_size_uses_compression_ratio_from_preferred_size() {
    let policy = BundleSizingPolicy::new(32 * 1024 * 1024, 16 * 1024 * 1024);
    let compressed_size = policy.snapshot_size_for(790 * 1024 * 1024);
    assert_eq!(compressed_size, 16 * 1024 * 1024);

    let raw_size = estimate_bundle_raw_capacity(compressed_size, 0.5, policy);
    assert_eq!(raw_size, 32 * 1024 * 1024);
}

#[test]
fn bundle_chunk_sort_groups_by_file_locality_segment_and_offset() {
    let file_a = ChunkHash::from_bytes([1; 32]);
    let file_b = ChunkHash::from_bytes([2; 32]);
    let chunk_a_next_segment = ChunkHash::from_bytes([3; 32]);
    let chunk_b = ChunkHash::from_bytes([4; 32]);
    let chunk_a_late = ChunkHash::from_bytes([5; 32]);
    let chunk_a_first = ChunkHash::from_bytes([6; 32]);
    let chunk_a_middle = ChunkHash::from_bytes([7; 32]);
    let mut chunks = vec![
        (
            chunk_b,
            ChunkPayload {
                source: std::sync::Arc::new(std::path::PathBuf::from("unused")),
                offset: 0,
                size: 1,
                hash: file_a,
            },
        ),
        (
            chunk_a_next_segment,
            ChunkPayload {
                source: std::sync::Arc::new(std::path::PathBuf::from("unused")),
                offset: 0,
                size: 1,
                hash: file_a,
            },
        ),
        (
            chunk_a_first,
            ChunkPayload {
                source: std::sync::Arc::new(std::path::PathBuf::from("unused")),
                offset: 0,
                size: 1,
                hash: file_a,
            },
        ),
        (
            chunk_a_middle,
            ChunkPayload {
                source: std::sync::Arc::new(std::path::PathBuf::from("unused")),
                offset: 0,
                size: 1,
                hash: file_a,
            },
        ),
        (
            chunk_a_late,
            ChunkPayload {
                source: std::sync::Arc::new(std::path::PathBuf::from("unused")),
                offset: 0,
                size: 1,
                hash: file_a,
            },
        ),
    ];
    let placements = HashMap::from([
        (
            chunk_b,
            ChunkPlacementKey {
                file_content_hash: file_b,
                file_offset: 0,
            },
        ),
        (
            chunk_a_next_segment,
            ChunkPlacementKey {
                file_content_hash: file_a,
                file_offset: 8,
            },
        ),
        (
            chunk_a_first,
            ChunkPlacementKey {
                file_content_hash: file_a,
                file_offset: 0,
            },
        ),
        (
            chunk_a_late,
            ChunkPlacementKey {
                file_content_hash: file_a,
                file_offset: 2,
            },
        ),
        (
            chunk_a_middle,
            ChunkPlacementKey {
                file_content_hash: file_a,
                file_offset: 1,
            },
        ),
    ]);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();

    sort_chunks_by_file_locality(&mut chunks, &placements, 4, &pool).unwrap();

    assert_eq!(
        chunks.iter().map(|(hash, _)| *hash).collect::<Vec<_>>(),
        vec![
            chunk_a_first,
            chunk_a_middle,
            chunk_a_late,
            chunk_b,
            chunk_a_next_segment,
        ]
    );
}

#[test]
fn source_range_payload_rejects_changes_between_scan_and_encoding() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::write(&source, b"before").unwrap();
    let payload = ChunkPayload {
        source: std::sync::Arc::new(source.clone()),
        offset: 0,
        size: 6,
        hash: ChunkHash::from_blake3(blake3::hash(b"before")),
    };
    assert_eq!(payload.load_bytes().unwrap(), b"before");
    std::fs::write(&source, b"after!").unwrap();
    assert!(payload.load_bytes().is_err());
    std::fs::write(&source, b"short").unwrap();
    assert!(payload.load_bytes().is_err());
}

#[test]
fn encoded_bundle_limit_includes_framing_and_splits_at_chunk_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let source = std::sync::Arc::new(root.path().join("source"));
    let bytes = [vec![b'a'; 32], vec![b'b'; 32]].concat();
    std::fs::write(source.as_ref(), &bytes).unwrap();
    let staging = root.path().join("artifacts");
    std::fs::create_dir(&staging).unwrap();
    let planned: Vec<_> = (0..2)
        .map(|index| {
            let hash = ChunkHash::from_blake3(blake3::hash(&bytes[index * 32..(index + 1) * 32]));
            (
                hash,
                ChunkPayload {
                    source: source.clone(),
                    offset: (index * 32) as u64,
                    size: 32,
                    hash,
                },
            )
        })
        .collect();
    let profile =
        manifest::CompressionProfile::for_algorithm(manifest::CompressionKind::None, None);
    let control = crate::progress::GutchOperationControl::default();
    let exact =
        build_bounded_bundle_groups(planned.clone(), 248, &profile, &staging, &control).unwrap();
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].bundle_size, 248);
    drop(exact);
    let split =
        build_bounded_bundle_groups(planned.clone(), 247, &profile, &staging, &control).unwrap();
    assert_eq!(split.len(), 2);
    assert!(
        split
            .iter()
            .all(|bundle| bundle.bundle_size == 180 && bundle.bundle_entries.len() == 1)
    );
    drop(split);
    assert!(
        build_bounded_bundle_groups(vec![planned[0].clone()], 179, &profile, &staging, &control)
            .is_err()
    );
    assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
}

#[test]
fn optimistic_probe_cannot_exceed_raw_bundle_cap() {
    assert_eq!(
        estimate_bundle_raw_capacity(
            256 * 1024 * 1024,
            0.00001,
            BundleSizingPolicy::new(256 * 1024 * 1024, 128 * 1024 * 1024)
        ),
        crate::artifact::format::MAX_RAW_BUNDLE_SIZE
    );
}
