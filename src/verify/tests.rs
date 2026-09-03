use super::*;
use crate::hash::ChunkHash;
use crate::manifest::{SnapshotTagSet, snapshot::SnapshotManifest};
use crate::test_util;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug)]
struct AlwaysCancelled;

impl crate::GutchCancellationHook for AlwaysCancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

fn test_chunk_hash(label: &str) -> ChunkHash {
    ChunkHash::from_blake3(crate::hash::hash_chunk(label.as_bytes()))
}

fn test_file_hash(label: &str) -> String {
    crate::hash::hash_hex(&crate::hash::hash_chunk(label.as_bytes()))
}

fn empty_manifest() -> SnapshotManifest {
    SnapshotManifest::new("t1".into(), SnapshotTagSet::default())
}

fn verify_with<'a>(
    snapshot_manifest: &'a SnapshotManifest,
    dir: &'a std::path::Path,
    mode: VerifyMode,
    runtime_hints: &'a RuntimeResourceHints,
) -> Result<(Vec<String>, VerifyTimings)> {
    let (classification, timings) = classify_directory(VerifyRequest {
        snapshot_manifest,
        dir,
        mode,
        runtime_hints,
        control: crate::GutchOperationControl::default(),
    })?;
    Ok((classification.into_messages(), timings))
}

fn verify_fast(
    snapshot_manifest: &SnapshotManifest,
    dir: &std::path::Path,
) -> Result<(Vec<String>, VerifyTimings)> {
    verify_with(
        snapshot_manifest,
        dir,
        VerifyMode::Fast,
        default_runtime_hints(),
    )
}

#[test]
fn pre_cancelled_verify_returns_typed_cancellation_before_io() {
    let missing_dir = test_util::temp_path("gutch_verify_pre_cancelled_missing");
    let _ = std::fs::remove_dir_all(&missing_dir);
    let result = classify_directory(VerifyRequest {
        snapshot_manifest: &empty_manifest(),
        dir: &missing_dir,
        mode: VerifyMode::Fast,
        runtime_hints: default_runtime_hints(),
        control: crate::GutchOperationControl::default()
            .with_cancellation(Arc::new(AlwaysCancelled)),
    });

    assert!(matches!(result, Err(crate::Error::Cancelled)));
    assert!(!missing_dir.exists());
}

#[test]
fn estimate_build_no_missing_chunks() {
    let manifest = empty_manifest();
    let available = HashSet::new();
    let result = estimate_build(&manifest, &available, 1_000_000).unwrap();
    assert_eq!(result.missing_chunk_count, 0);
    assert_eq!(result.missing_logical_bytes, 0);
    assert_eq!(result.required_bundle_count, 0);
}

#[test]
fn estimate_build_missing_chunks_counted() {
    let mut manifest = empty_manifest();
    manifest.push_file(crate::manifest::FileManifest {
        path: "game.exe".into(),
        size: 1000,
        executable: true,
        read_only: false,
        full_file_hash: test_file_hash("game.exe"),
        chunks: vec![
            crate::manifest::ChunkRef {
                chunk_hash: test_chunk_hash("a").to_hex(),
                raw_size: 500,
            },
            crate::manifest::ChunkRef {
                chunk_hash: test_chunk_hash("b").to_hex(),
                raw_size: 500,
            },
        ],
    });
    let available = HashSet::from([test_chunk_hash("a")]);
    let result = estimate_build(&manifest, &available, 1_000_000).unwrap();
    assert_eq!(result.missing_chunk_count, 1);
    assert_eq!(result.missing_logical_bytes, 500);
}

#[test]
fn estimate_build_all_chunks_available() {
    let mut manifest = empty_manifest();
    manifest.push_file(crate::manifest::FileManifest {
        path: "data.bin".into(),
        size: 200,
        executable: false,
        read_only: false,
        full_file_hash: test_file_hash("data.bin"),
        chunks: vec![
            crate::manifest::ChunkRef {
                chunk_hash: test_chunk_hash("x").to_hex(),
                raw_size: 100,
            },
            crate::manifest::ChunkRef {
                chunk_hash: test_chunk_hash("y").to_hex(),
                raw_size: 100,
            },
        ],
    });
    let available = HashSet::from([test_chunk_hash("x"), test_chunk_hash("y")]);
    let result = estimate_build(&manifest, &available, 500_000).unwrap();
    assert_eq!(result.missing_chunk_count, 0);
    assert_eq!(result.local_cache_hits, 0);
}

#[test]
fn estimate_build_available_disk_space_passed_through() {
    let manifest = empty_manifest();
    let available = HashSet::new();
    let result = estimate_build(&manifest, &available, 42_000).unwrap();
    assert_eq!(result.available_disk_space, 42_000);
}

#[test]
fn verify_contiguous_chunks_detects_corruption() {
    let dir = test_util::temp_path("gutch_verify_contiguous_test");
    test_util::recreate_dir(&dir);

    let chunk_a_data = b"AAAAA";
    let chunk_b_data = b"BBBBB";
    let mut full_file = Vec::new();
    full_file.extend_from_slice(chunk_a_data);
    full_file.extend_from_slice(chunk_b_data);
    let file_hash = crate::hash::hash_hex(&crate::hash::hash_chunk(&full_file));

    std::fs::write(dir.join("data.bin"), &full_file).unwrap();

    let mut manifest = empty_manifest();
    manifest.push_file(crate::manifest::FileManifest {
        path: "data.bin".into(),
        size: full_file.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: file_hash,
        chunks: vec![
            crate::manifest::ChunkRef {
                chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(chunk_a_data)),
                raw_size: chunk_a_data.len() as u64,
            },
            crate::manifest::ChunkRef {
                chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(chunk_b_data)),
                raw_size: chunk_b_data.len() as u64,
            },
        ],
    });
    let (errors, _) = verify_fast(&manifest, &dir).unwrap();
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);

    // Now corrupt the file — swap data
    let mut corrupted = Vec::new();
    corrupted.extend_from_slice(chunk_b_data);
    corrupted.extend_from_slice(chunk_a_data);
    std::fs::write(dir.join("data.bin"), &corrupted).unwrap();
    let (errors, _) = verify_fast(&manifest, &dir).unwrap();
    assert!(!errors.is_empty(), "expected corruption errors");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_contiguous_chunks_single_chunk() {
    let dir = test_util::temp_path("gutch_verify_single_chunk");
    test_util::recreate_dir(&dir);

    let data = b"single chunk content here";
    let file_hash = crate::hash::hash_hex(&crate::hash::hash_chunk(data));
    std::fs::write(dir.join("single.bin"), data).unwrap();

    let mut manifest = empty_manifest();
    manifest.push_file(crate::manifest::FileManifest {
        path: "single.bin".into(),
        size: data.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: file_hash,
        chunks: vec![crate::manifest::ChunkRef {
            chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(data)),
            raw_size: data.len() as u64,
        }],
    });
    let (errors, _) = verify_fast(&manifest, &dir).unwrap();
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_default_fast_mode_skips_chunk_hashing() {
    let dir = test_util::temp_path("gutch_verify_fast_mode");
    test_util::recreate_dir(&dir);

    let data = b"fast mode content";
    let file_hash = crate::hash::hash_hex(&crate::hash::hash_chunk(data));
    std::fs::write(dir.join("fast.bin"), data).unwrap();

    let mut manifest = empty_manifest();
    manifest.push_file(crate::manifest::FileManifest {
        path: "fast.bin".into(),
        size: data.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: file_hash,
        chunks: vec![crate::manifest::ChunkRef {
            chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(data)),
            raw_size: data.len() as u64,
        }],
    });

    let (errors, timings) = verify_fast(&manifest, &dir).unwrap();
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    assert_eq!(timings.verify_mode, VerifyMode::Fast);
    assert_eq!(timings.chunk_hash_bytes, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn expected_entry_classification_ignores_unrelated_extra_files() {
    let dir = test_util::temp_path("gutch_verify_expected_entries_only");
    test_util::recreate_dir(&dir);

    let expected = b"expected";
    std::fs::write(dir.join("expected.bin"), b"corrupt!").unwrap();
    std::fs::write(dir.join("unrelated.txt"), b"user data").unwrap();
    let mut manifest = empty_manifest();
    manifest.push_file(crate::manifest::FileManifest {
        path: "expected.bin".into(),
        size: expected.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(expected)),
        chunks: Vec::new(),
    });

    let (classification, _) = super::classify_expected_entries_with_marker_namespace(
        super::VerifyRequest {
            snapshot_manifest: &manifest,
            dir: &dir,
            mode: VerifyMode::Fast,
            runtime_hints: default_runtime_hints(),
            control: crate::GutchOperationControl::default(),
        },
        "reconstructed.verify",
    )
    .unwrap();

    assert_eq!(classification.corrupt_files.len(), 1);
    assert!(classification.extra_files.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_strict_mode_checks_chunk_hashing() {
    let dir = test_util::temp_path("gutch_verify_strict_mode");
    test_util::recreate_dir(&dir);

    let chunk_a_data = b"AAAAA";
    let chunk_b_data = b"BBBBB";
    let mut full_file = Vec::new();
    full_file.extend_from_slice(chunk_a_data);
    full_file.extend_from_slice(chunk_b_data);
    let file_hash = crate::hash::hash_hex(&crate::hash::hash_chunk(&full_file));

    std::fs::write(dir.join("strict.bin"), &full_file).unwrap();

    let mut manifest = empty_manifest();
    manifest.push_file(crate::manifest::FileManifest {
        path: "strict.bin".into(),
        size: full_file.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: file_hash,
        chunks: vec![
            crate::manifest::ChunkRef {
                chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(chunk_b_data)),
                raw_size: chunk_a_data.len() as u64,
            },
            crate::manifest::ChunkRef {
                chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(chunk_a_data)),
                raw_size: chunk_b_data.len() as u64,
            },
        ],
    });

    let (fast_errors, fast_timings) = verify_fast(&manifest, &dir).unwrap();
    assert!(
        fast_errors.is_empty(),
        "fast mode should trust the full-file hash, got: {:?}",
        fast_errors
    );
    assert_eq!(fast_timings.chunk_hash_bytes, 0);

    let (strict_errors, strict_timings) =
        verify_with(&manifest, &dir, VerifyMode::Strict, default_runtime_hints()).unwrap();
    assert_eq!(strict_timings.verify_mode, VerifyMode::Strict);
    assert_eq!(strict_timings.chunk_hash_bytes, full_file.len() as u64);
    assert!(
        strict_errors
            .iter()
            .any(|error| error.contains("chunk hash mismatch")),
        "strict mode should report chunk hash mismatches, got: {:?}",
        strict_errors
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn estimate_build_empty_files_no_chunks() {
    let manifest = empty_manifest();
    let available = HashSet::new();
    let result = estimate_build(&manifest, &available, 0).unwrap();
    assert_eq!(result.missing_chunk_count, 0);
    assert_eq!(result.required_bundle_count, 0);
}

#[test]
fn verify_rejects_manifest_path_traversal() {
    let dir = test_util::temp_path("gutch_verify_path_traversal");
    test_util::recreate_dir(&dir);

    let mut manifest = empty_manifest();
    manifest.push_file(crate::manifest::FileManifest {
        path: "../escape.txt".into(),
        size: 1,
        executable: false,
        read_only: false,
        full_file_hash: "hash".into(),
        chunks: vec![],
    });

    let error = verify_fast(&manifest, &dir).unwrap_err();
    assert!(matches!(error, crate::Error::PathError { .. }));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_with_hints_records_runtime_plan() {
    let dir = test_util::temp_path("gutch_verify_runtime_plan");
    test_util::recreate_dir(&dir);

    let data = b"runtime plan content";
    let file_hash = crate::hash::hash_hex(&crate::hash::hash_chunk(data));
    std::fs::write(dir.join("runtime.bin"), data).unwrap();

    let mut manifest = empty_manifest();
    manifest.push_file(crate::manifest::FileManifest {
        path: "runtime.bin".into(),
        size: data.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: file_hash,
        chunks: vec![crate::manifest::ChunkRef {
            chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(data)),
            raw_size: data.len() as u64,
        }],
    });

    let (_, timings) = verify_with(
        &manifest,
        &dir,
        VerifyMode::Fast,
        &crate::runtime::RuntimeResourceHints { jobs: Some(1) },
    )
    .unwrap();

    assert_eq!(
        timings.runtime_plan.as_ref().map(|plan| plan.worker_count),
        Some(1)
    );
    assert!(
        timings.total
            >= timings.prepare_wall + timings.verify_files_wall + timings.extra_file_scan_wall
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn verify_inventory_supports_symlinked_parent_inside_root() {
    use std::os::unix::fs::symlink;

    let dir = test_util::temp_path("gutch_verify_inventory_symlink_parent");
    test_util::recreate_dir(&dir);
    std::fs::create_dir(dir.join("real")).unwrap();
    std::fs::write(dir.join("real/data.bin"), b"symlink parent").unwrap();
    symlink("real", dir.join("link")).unwrap();

    let hash = crate::hash::hash_hex(&crate::hash::hash_chunk(b"symlink parent"));
    let mut manifest = empty_manifest();
    manifest.entries = vec![
        crate::manifest::SnapshotEntry::Directory(crate::manifest::DirectoryManifest {
            path: "real".into(),
            read_only: false,
        }),
        crate::manifest::SnapshotEntry::Symlink(crate::manifest::SymlinkManifest {
            path: "link".into(),
            target_path: "real".into(),
        }),
        crate::manifest::SnapshotEntry::File(crate::manifest::FileManifest {
            path: "real/data.bin".into(),
            size: 14,
            executable: false,
            read_only: false,
            full_file_hash: hash.clone(),
            chunks: Vec::new(),
        }),
        crate::manifest::SnapshotEntry::File(crate::manifest::FileManifest {
            path: "link/data.bin".into(),
            size: 14,
            executable: false,
            read_only: false,
            full_file_hash: hash,
            chunks: Vec::new(),
        }),
    ];

    let (errors, _) = verify_fast(&manifest, &dir).unwrap();
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn serpentine_worker_lanes_alternate_direction_each_round() {
    let assignments = (0..9)
        .map(|sorted_index| serpentine_worker_index(sorted_index, 3))
        .collect::<Vec<_>>();

    assert_eq!(assignments, vec![0, 1, 2, 2, 1, 0, 0, 1, 2]);
}

#[test]
fn serpentine_worker_lane_supports_one_worker() {
    assert!((0..8).all(|sorted_index| serpentine_worker_index(sorted_index, 1) == 0));
}
