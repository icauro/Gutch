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

    let raw_size = raw_size_size_for_bundle_compression(compressed_size, 0.5, policy);
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
            ChunkPayload::InMemory {
                bytes: std::sync::Arc::<[u8]>::from(vec![0u8]),
            },
        ),
        (
            chunk_a_next_segment,
            ChunkPayload::InMemory {
                bytes: std::sync::Arc::<[u8]>::from(vec![0u8]),
            },
        ),
        (
            chunk_a_first,
            ChunkPayload::InMemory {
                bytes: std::sync::Arc::<[u8]>::from(vec![0u8]),
            },
        ),
        (
            chunk_a_middle,
            ChunkPayload::InMemory {
                bytes: std::sync::Arc::<[u8]>::from(vec![0u8]),
            },
        ),
        (
            chunk_a_late,
            ChunkPayload::InMemory {
                bytes: std::sync::Arc::<[u8]>::from(vec![0u8]),
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
