use super::*;

#[test]
fn bundle_sizing_policy_uses_preferred_encoded_target() {
    assert_eq!(BundleSizingPolicy::new(100, 10).encoded_target_size(), 10);
    assert_eq!(BundleSizingPolicy::new(100, 200).encoded_target_size(), 100);
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
    let compressed_size = policy.encoded_target_size();
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
    let profile = crate::artifact::profiles::CompressionProfile::for_algorithm(
        crate::artifact::profiles::CompressionKind::None,
        None,
    );
    let control = crate::progress::GutchOperationControl::default();
    let exact =
        build_bounded_bundle_groups(planned.clone(), 216, &profile, &staging, &control).unwrap();
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].bundle_size, 216);
    drop(exact);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let placements = planned
        .iter()
        .map(|(hash, payload)| {
            (
                *hash,
                ChunkPlacementKey {
                    file_content_hash: payload.hash,
                    file_offset: payload.offset,
                },
            )
        })
        .collect();
    let plan = plan_bundle_compressed_groups(
        planned.clone(),
        &placements,
        &profile,
        &crate::artifact::profiles::BundleSizingProfile::default(),
        BundleSizingPolicy::new(215, 64),
        &pool,
        &control,
    )
    .unwrap();
    assert_eq!(plan.groups.len(), 1);
    assert_eq!(plan.probe_raw_bytes, 0);
    let split =
        build_bundled_chunks(plan, &profile, &staging, &pool, &control, 512 * 1024 * 1024).unwrap();
    assert_eq!(split.len(), 2);
    assert!(
        split
            .iter()
            .all(|bundle| bundle.bundle_size == 148 && bundle.bundle_entries.len() == 1)
    );
    drop(split);
    assert!(
        build_bounded_bundle_groups(vec![planned[0].clone()], 147, &profile, &staging, &control)
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

#[test]
fn memory_admission_keeps_capture_results_in_input_order() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    let control = crate::progress::GutchOperationControl::default();
    let result = super::run_parallel_chunk_work(
        (0..32).collect(),
        &pool,
        &control,
        1,
        |_| 1,
        |index, _| Ok(index),
    )
    .unwrap();
    assert_eq!(result, (0..32).collect::<Vec<_>>());
    let result =
        super::run_parallel_chunk_work(vec![0, 1], &pool, &control, 1, |_| 2, |index, _| Ok(index));
    assert!(result.is_err());
}

#[test]
fn payload_reader_seeks_switches_sources_and_detects_mutation() {
    use crate::capture::types::ChunkPayloadReader;
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    std::fs::write(&first, b"abcdef").unwrap();
    std::fs::write(&second, b"uvwxyz").unwrap();
    let chunk = |path: &std::path::Path, offset, bytes: &[u8]| ChunkPayload {
        source: std::sync::Arc::new(path.to_path_buf()),
        offset,
        size: bytes.len() as u64,
        hash: ChunkHash::from_blake3(blake3::hash(bytes)),
    };
    let mut reader = ChunkPayloadReader::default();
    for payload in [
        chunk(&first, 0, b"abc"),
        chunk(&first, 3, b"def"),
        chunk(&first, 0, b"abc"),
        chunk(&second, 1, b"vw"),
        chunk(&first, 2, b"cd"),
    ] {
        assert_eq!(
            blake3::hash(&reader.load(&payload).unwrap()),
            blake3::hash(
                &std::fs::read(payload.source.as_ref()).unwrap()
                    [payload.offset as usize..(payload.offset + payload.size) as usize]
            )
        );
    }
    let expected = chunk(&first, 0, b"abc");
    std::fs::write(&first, b"ABCdef").unwrap();
    assert!(reader.load(&expected).is_err());
    std::fs::write(&first, b"a").unwrap();
    assert!(reader.load(&expected).is_err());
    std::fs::write(&first, b"abcdef").unwrap();
    assert_eq!(reader.load(&expected).unwrap(), b"abc");
}

#[test]
fn uncompressed_probe_uses_exact_lengths_without_loading_sources() {
    let temp = tempfile::tempdir().unwrap();
    let segments: [&[u8]; 3] = [b"ab", b"cdef", b"g"];
    let chunks: Vec<_> = segments
        .iter()
        .map(|bytes| {
            let hash = ChunkHash::from_blake3(blake3::hash(bytes));
            (
                hash,
                ChunkPayload {
                    source: std::sync::Arc::new(temp.path().join("missing")),
                    offset: 0,
                    size: bytes.len() as u64,
                    hash,
                },
            )
        })
        .collect();
    let profile = crate::artifact::profiles::CompressionProfile::for_algorithm(
        crate::artifact::profiles::CompressionKind::None,
        None,
    );
    for (target, selected) in [(0, 1), (2, 1), (3, 2), (6, 2), (7, 3), (99, 3)] {
        let selected_segments = &segments[..selected];
        let raw: usize = selected_segments.iter().map(|s| s.len()).sum();
        let encoded = compress::compressed_size(selected_segments, &profile).unwrap()
            + crate::artifact::bundle::encoded_metadata_size(selected as u64);
        let (ratio, read_bytes) =
            estimate_bundle_compression_ratio_from_probe(&chunks, &profile, target).unwrap();
        assert_eq!(ratio, encoded as f64 / raw as f64);
        assert_eq!(read_bytes, 0);
    }
    assert_eq!(
        estimate_bundle_compression_ratio_from_probe(&[], &profile, 16).unwrap(),
        (1.0, 0)
    );
    // Skipping an unnecessary probe must not skip loading during actual encoding.
    assert!(
        build_bundle_compressed_group(chunks, &profile, temp.path(), &Default::default()).is_err()
    );
}

#[test]
fn capture_result_slots_join_workers_before_returning_an_error() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    let completed = std::sync::atomic::AtomicUsize::new(0);
    let result = super::run_parallel_chunk_work(
        (0..32).collect(),
        &pool,
        &crate::progress::GutchOperationControl::default(),
        4,
        |_| 1,
        |index, _| {
            completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if index == 3 {
                Err(crate::Error::manifest("worker failed"))
            } else {
                Ok(index)
            }
        },
    );
    assert!(result.is_err());
    assert_eq!(completed.load(std::sync::atomic::Ordering::Relaxed), 32);
}

#[test]
fn single_raw_group_skips_probe_but_keeps_placement_and_source_checks() {
    use crate::artifact::profiles::{BundleSizingProfile, CompressionKind, CompressionProfile};
    let directory = tempfile::tempdir().unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let chunks = [
        b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    ]
    .iter()
    .map(|bytes| {
        let hash = ChunkHash::from_blake3(blake3::hash(*bytes));
        (
            hash,
            ChunkPayload {
                source: std::sync::Arc::new(directory.path().join("missing")),
                offset: 0,
                size: 32,
                hash,
            },
        )
    })
    .collect::<Vec<_>>();
    let placements = chunks
        .iter()
        .enumerate()
        .map(|(index, (hash, _))| {
            (
                *hash,
                ChunkPlacementKey {
                    file_content_hash: *hash,
                    file_offset: index as u64 * 32,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let bundle_profile = BundleSizingProfile::default();
    let control = crate::progress::GutchOperationControl::default();
    for codec in [
        CompressionKind::None,
        CompressionKind::Zstd,
        CompressionKind::Lz4,
    ] {
        let profile = CompressionProfile::for_algorithm(codec, None);
        // The payload equals the raw floor; planning must not open the missing source.
        let plan = plan_bundle_compressed_groups(
            chunks.clone(),
            &placements,
            &profile,
            &bundle_profile,
            BundleSizingPolicy::new(215, 64),
            &pool,
            &control,
        )
        .unwrap();
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(plan.groups[0].len(), 2);
        assert_eq!(plan.probe_raw_bytes, 0);
        assert_eq!(plan.encoded_limit, 215);
        let expected_order = plan.groups[0]
            .iter()
            .map(|(hash, _)| *hash)
            .collect::<Vec<_>>();
        let mut reversed = chunks.clone();
        reversed.reverse();
        let reversed_plan = plan_bundle_compressed_groups(
            reversed,
            &placements,
            &profile,
            &bundle_profile,
            BundleSizingPolicy::new(215, 64),
            &pool,
            &control,
        )
        .unwrap();
        assert_eq!(
            expected_order,
            reversed_plan.groups[0]
                .iter()
                .map(|(hash, _)| *hash)
                .collect::<Vec<_>>()
        );
        assert!(
            build_bundle_compressed_group(
                plan.groups.into_iter().next().unwrap(),
                &profile,
                directory.path(),
                &control
            )
            .is_err()
        );
        assert!(
            plan_bundle_compressed_groups(
                chunks.clone(),
                &HashMap::new(),
                &profile,
                &bundle_profile,
                BundleSizingPolicy::new(215, 64),
                &pool,
                &control
            )
            .is_err()
        );
        // One byte above the floor still uses the normal sizing/probe path.
        let above_floor = plan_bundle_compressed_groups(
            chunks.clone(),
            &placements,
            &profile,
            &bundle_profile,
            BundleSizingPolicy::new(215, 63),
            &pool,
            &control,
        );
        if codec == CompressionKind::None {
            assert_eq!(above_floor.unwrap().groups.len(), 2);
        } else {
            assert!(
                above_floor.is_err(),
                "compressed probe must open the source above the floor"
            );
        }
    }
}

#[test]
fn source_change_after_single_group_planning_fails_encoding() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    std::fs::write(&source, b"before").unwrap();
    let hash = ChunkHash::from_blake3(blake3::hash(b"before"));
    let chunks = vec![(
        hash,
        ChunkPayload {
            source: std::sync::Arc::new(source.clone()),
            offset: 0,
            size: 6,
            hash,
        },
    )];
    let placements = HashMap::from([(
        hash,
        ChunkPlacementKey {
            file_content_hash: hash,
            file_offset: 0,
        },
    )]);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let profile = crate::artifact::profiles::CompressionProfile::default();
    let control = crate::progress::GutchOperationControl::default();
    let plan = plan_bundle_compressed_groups(
        chunks,
        &placements,
        &profile,
        &crate::artifact::profiles::BundleSizingProfile::default(),
        BundleSizingPolicy::default(),
        &pool,
        &control,
    )
    .unwrap();
    std::fs::write(source, b"after!").unwrap();
    assert!(
        build_bundle_compressed_group(
            plan.groups.into_iter().next().unwrap(),
            &profile,
            directory.path(),
            &control
        )
        .is_err()
    );
}
