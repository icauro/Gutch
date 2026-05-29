use super::*;
use crate::manifest::{TargetTagSet, target::TargetManifest};
use crate::test_util;

fn empty_manifest() -> TargetManifest {
    TargetManifest::new("t1".into(), TargetTagSet::default())
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
    manifest.files.push(crate::manifest::FileManifest {
        path: "game.exe".into(),
        size: 1000,
        executable: true,
        full_file_hash: "hash1".into(),
        chunks: vec![
            crate::manifest::ChunkRef {
                chunk_hash: "a".into(),
                raw_size: 500,
            },
            crate::manifest::ChunkRef {
                chunk_hash: "b".into(),
                raw_size: 500,
            },
        ],
    });
    let available = HashSet::from(["a".into()]);
    let result = estimate_build(&manifest, &available, 1_000_000).unwrap();
    assert_eq!(result.missing_chunk_count, 1);
    assert_eq!(result.missing_logical_bytes, 500);
}

#[test]
fn estimate_build_all_chunks_available() {
    let mut manifest = empty_manifest();
    manifest.files.push(crate::manifest::FileManifest {
        path: "data.bin".into(),
        size: 200,
        executable: false,
        full_file_hash: "hash2".into(),
        chunks: vec![
            crate::manifest::ChunkRef {
                chunk_hash: "x".into(),
                raw_size: 100,
            },
            crate::manifest::ChunkRef {
                chunk_hash: "y".into(),
                raw_size: 100,
            },
        ],
    });
    let available = HashSet::from(["x".into(), "y".into()]);
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
    manifest.files.push(crate::manifest::FileManifest {
        path: "data.bin".into(),
        size: full_file.len() as u64,
        executable: false,
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
    let errors = verify_directory(&manifest, &dir).unwrap();
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);

    // Now corrupt the file — swap data
    let mut corrupted = Vec::new();
    corrupted.extend_from_slice(chunk_b_data);
    corrupted.extend_from_slice(chunk_a_data);
    std::fs::write(dir.join("data.bin"), &corrupted).unwrap();
    let errors = verify_directory(&manifest, &dir).unwrap();
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
    manifest.files.push(crate::manifest::FileManifest {
        path: "single.bin".into(),
        size: data.len() as u64,
        executable: false,
        full_file_hash: file_hash,
        chunks: vec![crate::manifest::ChunkRef {
            chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(data)),
            raw_size: data.len() as u64,
        }],
    });
    let errors = verify_directory(&manifest, &dir).unwrap();
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);

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
    manifest.files.push(crate::manifest::FileManifest {
        path: "../escape.txt".into(),
        size: 1,
        executable: false,
        full_file_hash: "hash".into(),
        chunks: vec![],
    });

    let error = verify_directory(&manifest, &dir).unwrap_err();
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
    manifest.files.push(crate::manifest::FileManifest {
        path: "runtime.bin".into(),
        size: data.len() as u64,
        executable: false,
        full_file_hash: file_hash,
        chunks: vec![crate::manifest::ChunkRef {
            chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(data)),
            raw_size: data.len() as u64,
        }],
    });

    let (_, timings) = verify_directory_with_timings_and_hints(
        &manifest,
        &dir,
        &crate::runtime::RuntimeResourceHints {
            memory_mode: crate::runtime::MemoryMode::Low,
            max_memory_mib: Some(1024),
            jobs: Some(1),
        },
    )
    .unwrap();

    assert_eq!(
        timings.runtime_plan.as_ref().map(|plan| plan.worker_count),
        Some(1)
    );

    let _ = std::fs::remove_dir_all(&dir);
}
