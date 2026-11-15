use super::*;
use crate::artifact::hash::ChunkHash;
use crate::manifest::ChunkRef;
use crate::manifest::FileManifest;
use crate::manifest::SnapshotTagSet;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::CancellationHook;
use crate::progress::OperationControl;
use crate::runtime::RuntimeResourceHints;
use crate::test_util;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

fn default_runtime_hints() -> &'static RuntimeResourceHints {
    static DEFAULT_HINTS: OnceLock<RuntimeResourceHints> = OnceLock::new();
    DEFAULT_HINTS.get_or_init(RuntimeResourceHints::default)
}

#[derive(Debug)]
struct AlwaysCancelled;

impl CancellationHook for AlwaysCancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

fn empty_manifest() -> SnapshotManifest {
    SnapshotManifest::new("t1".into(), SnapshotTagSet::default())
}

fn verify_with<'a>(
    snapshot_manifest: &'a SnapshotManifest,
    dir: &'a Path,
    mode: VerifyMode,
    runtime_hints: &'a RuntimeResourceHints,
) -> Result<(Vec<String>, VerifyTimings)> {
    let (classification, timings) = classify_directory(VerifyRequest {
        snapshot_manifest,
        dir,
        mode,
        runtime_hints,
        control: OperationControl::default(),
        spans: &InspectionSpans::VERIFY,
        pool: None,
    })?;
    Ok((classification.into_messages(), timings))
}

fn verify_fast(
    snapshot_manifest: &SnapshotManifest,
    dir: &Path,
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
    let _ = fs::remove_dir_all(&missing_dir);
    let result = classify_directory(VerifyRequest {
        snapshot_manifest: &empty_manifest(),
        dir: &missing_dir,
        mode: VerifyMode::Fast,
        runtime_hints: default_runtime_hints(),
        control: OperationControl::default().with_cancellation(Arc::new(AlwaysCancelled)),
        spans: &InspectionSpans::VERIFY,
        pool: None,
    });

    assert!(matches!(result, Err(crate::Error::Cancelled)));
    assert!(!missing_dir.exists());
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
    let file_hash = ChunkHash::of(&full_file).to_hex();

    fs::write(dir.join("data.bin"), &full_file).unwrap();

    let mut manifest = empty_manifest();
    manifest.push_file(FileManifest {
        path: "data.bin".into(),
        size: full_file.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: file_hash,
        chunks: vec![
            ChunkRef {
                chunk_hash: ChunkHash::parse_hex(&ChunkHash::of(chunk_a_data).to_hex()).unwrap(),
                raw_size: chunk_a_data.len() as u32,
            },
            ChunkRef {
                chunk_hash: ChunkHash::parse_hex(&ChunkHash::of(chunk_b_data).to_hex()).unwrap(),
                raw_size: chunk_b_data.len() as u32,
            },
        ],
    });
    let (errors, _) = verify_fast(&manifest, &dir).unwrap();
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);

    // Now corrupt the file — swap data
    let mut corrupted = Vec::new();
    corrupted.extend_from_slice(chunk_b_data);
    corrupted.extend_from_slice(chunk_a_data);
    fs::write(dir.join("data.bin"), &corrupted).unwrap();
    let (errors, _) = verify_fast(&manifest, &dir).unwrap();
    assert!(!errors.is_empty(), "expected corruption errors");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn verify_contiguous_chunks_single_chunk() {
    let dir = test_util::temp_path("gutch_verify_single_chunk");
    test_util::recreate_dir(&dir);

    let data = b"single chunk content here";
    let file_hash = ChunkHash::of(data).to_hex();
    fs::write(dir.join("single.bin"), data).unwrap();

    let mut manifest = empty_manifest();
    manifest.push_file(FileManifest {
        path: "single.bin".into(),
        size: data.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: file_hash,
        chunks: vec![ChunkRef {
            chunk_hash: ChunkHash::parse_hex(&ChunkHash::of(data).to_hex()).unwrap(),
            raw_size: data.len() as u32,
        }],
    });
    let (errors, _) = verify_fast(&manifest, &dir).unwrap();
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn verify_default_fast_mode_skips_chunk_hashing() {
    let dir = test_util::temp_path("gutch_verify_fast_mode");
    test_util::recreate_dir(&dir);

    let data = b"fast mode content";
    let file_hash = ChunkHash::of(data).to_hex();
    fs::write(dir.join("fast.bin"), data).unwrap();

    let mut manifest = empty_manifest();
    manifest.push_file(FileManifest {
        path: "fast.bin".into(),
        size: data.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: file_hash,
        chunks: vec![ChunkRef {
            chunk_hash: ChunkHash::parse_hex(&ChunkHash::of(data).to_hex()).unwrap(),
            raw_size: data.len() as u32,
        }],
    });

    let (errors, timings) = verify_fast(&manifest, &dir).unwrap();
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    assert_eq!(timings.verify_mode, VerifyMode::Fast);
    assert_eq!(timings.chunk_hash_bytes, 0);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn expected_entry_classification_ignores_unrelated_extra_files() {
    let dir = test_util::temp_path("gutch_verify_expected_entries_only");
    test_util::recreate_dir(&dir);

    let expected = b"expected";
    fs::write(dir.join("expected.bin"), b"corrupt!").unwrap();
    fs::write(dir.join("unrelated.txt"), b"user data").unwrap();
    let mut manifest = empty_manifest();
    manifest.push_file(FileManifest {
        path: "expected.bin".into(),
        size: expected.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: ChunkHash::of(expected).to_hex(),
        chunks: Vec::new(),
    });

    let (classification, _) = super::classify_expected_entries(super::VerifyRequest {
        snapshot_manifest: &manifest,
        dir: &dir,
        mode: VerifyMode::Fast,
        runtime_hints: default_runtime_hints(),
        control: OperationControl::default(),
        spans: &InspectionSpans::VERIFY,
        pool: None,
    })
    .unwrap();

    assert_eq!(classification.corrupt_files.len(), 1);
    assert!(classification.extra_files.is_empty());
    let _ = fs::remove_dir_all(&dir);
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
    let file_hash = ChunkHash::of(&full_file).to_hex();

    fs::write(dir.join("strict.bin"), &full_file).unwrap();

    let mut manifest = empty_manifest();
    manifest.push_file(FileManifest {
        path: "strict.bin".into(),
        size: full_file.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: file_hash,
        chunks: vec![
            ChunkRef {
                chunk_hash: ChunkHash::parse_hex(&ChunkHash::of(chunk_b_data).to_hex()).unwrap(),
                raw_size: chunk_a_data.len() as u32,
            },
            ChunkRef {
                chunk_hash: ChunkHash::parse_hex(&ChunkHash::of(chunk_a_data).to_hex()).unwrap(),
                raw_size: chunk_b_data.len() as u32,
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

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn verify_rejects_manifest_path_traversal() {
    let dir = test_util::temp_path("gutch_verify_path_traversal");
    test_util::recreate_dir(&dir);

    let mut manifest = empty_manifest();
    manifest.push_file(FileManifest {
        path: "../escape.txt".into(),
        size: 1,
        executable: false,
        read_only: false,
        full_file_hash: "hash".into(),
        chunks: vec![],
    });

    let error = verify_fast(&manifest, &dir).unwrap_err();
    assert!(matches!(error, crate::Error::PathError { .. }));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn verify_with_hints_records_runtime_plan() {
    let dir = test_util::temp_path("gutch_verify_runtime_plan");
    test_util::recreate_dir(&dir);

    let data = b"runtime plan content";
    let file_hash = ChunkHash::of(data).to_hex();
    fs::write(dir.join("runtime.bin"), data).unwrap();

    let mut manifest = empty_manifest();
    manifest.push_file(FileManifest {
        path: "runtime.bin".into(),
        size: data.len() as u64,
        executable: false,
        read_only: false,
        full_file_hash: file_hash,
        chunks: vec![ChunkRef {
            chunk_hash: ChunkHash::parse_hex(&ChunkHash::of(data).to_hex()).unwrap(),
            raw_size: data.len() as u32,
        }],
    });

    let (_, timings) = verify_with(
        &manifest,
        &dir,
        VerifyMode::Fast,
        &RuntimeResourceHints {
            jobs: Some(1),
            ..Default::default()
        },
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

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn verify_inventory_supports_symlinked_parent_inside_root() {
    use std::os::unix::fs::symlink;

    let dir = test_util::temp_path("gutch_verify_inventory_symlink_parent");
    test_util::recreate_dir(&dir);
    fs::create_dir(dir.join("real")).unwrap();
    fs::write(dir.join("real/data.bin"), b"symlink parent").unwrap();
    symlink("real", dir.join("link")).unwrap();

    let hash = ChunkHash::of(b"symlink parent").to_hex();
    let mut manifest = empty_manifest();
    manifest.entries = vec![
        SnapshotEntry::Directory(DirectoryManifest {
            path: "real".into(),
            read_only: false,
        }),
        SnapshotEntry::Symlink(SymlinkManifest {
            path: "link".into(),
            target_path: "real".into(),
        }),
        SnapshotEntry::File(FileManifest {
            path: "real/data.bin".into(),
            size: 14,
            executable: false,
            read_only: false,
            full_file_hash: hash.clone(),
            chunks: Vec::new(),
        }),
        SnapshotEntry::File(FileManifest {
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

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn parallel_verification_preserves_issue_order_across_worker_counts() {
    let dir = test_util::temp_path("gutch_verify_parallel_issue_order");
    test_util::recreate_dir(&dir);
    let mut manifest = empty_manifest();
    // Include equal-sized entries and reverse manifest order to exercise both sort keys.
    for (name, size) in [("z.bin", 32), ("b.bin", 64), ("a.bin", 64), ("c.bin", 128)] {
        let expected = vec![1_u8; size];
        fs::write(dir.join(name), vec![2_u8; size]).unwrap();
        manifest.push_file(FileManifest {
            path: name.into(),
            size: size as u64,
            executable: false,
            read_only: false,
            full_file_hash: ChunkHash::of(&expected).to_hex(),
            chunks: Vec::new(),
        });
    }
    let mut expected_messages = None;
    for workers in [1, 2, 4] {
        let (classification, _) = classify_directory(VerifyRequest {
            snapshot_manifest: &manifest,
            dir: &dir,
            mode: VerifyMode::Fast,
            runtime_hints: &RuntimeResourceHints {
                jobs: Some(workers),
                ..Default::default()
            },
            control: OperationControl::default(),
            spans: &InspectionSpans::VERIFY,
            pool: None,
        })
        .unwrap();
        let paths = classification
            .corrupt_files
            .iter()
            .map(|issue| issue.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["c.bin", "a.bin", "b.bin", "z.bin"]);
        let messages = classification.into_messages();
        if let Some(expected) = &expected_messages {
            assert_eq!(&messages, expected);
        } else {
            expected_messages = Some(messages);
        }
    }
    fs::remove_dir_all(&dir).unwrap();
}
