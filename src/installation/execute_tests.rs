use super::*;
use crate::artifact::hash::hash_hex;
use crate::installation::types::SnapshotReconstructSource;
use crate::manifest;
use crate::manifest::ChunkRef;
use crate::manifest::FileManifest;
use crate::manifest::SnapshotManifest;
use crate::manifest::SnapshotTagSet;
use std::collections::BTreeMap;

#[test]
fn output_verification_uses_file_concurrency_without_misreporting_pool_size() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large.bin");
    let bytes = vec![37; crate::file_integrity::PARALLEL_FULL_HASH_MIN_BYTES as usize];
    std::fs::write(&path, &bytes).unwrap();
    let file_plan = ReconstructPipelineFile {
        path: "large.bin".into(),
        size: bytes.len() as u64,
        full_file_hash: crate::artifact::hash::ChunkHash::from_blake3(blake3::hash(&bytes)),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    for (file_count, expected_mode) in [(1, "mmap_rayon"), (2, "mmap")] {
        let profiler = Arc::new(crate::profile::GutchProfiler::new(
            crate::profile::GutchProfileMode::Detailed,
        ));
        let control = GutchOperationControl::default().with_profiler(profiler.clone());
        let mut paths = vec![path.clone()];
        if file_count == 2 {
            paths.push(directory.path().join("another-planned-file"));
        }
        pool.install(|| {
            verify_reconstructed_output_file(
                0, &file_plan, &paths, &[Mutex::new(None)], &control, 4,
            )
        })
        .unwrap();
        let trace_path = directory.path().join("trace.json");
        profiler.write_trace(&trace_path).unwrap();
        let trace: serde_json::Value =
            serde_json::from_slice(&std::fs::read(trace_path).unwrap()).unwrap();
        let event = trace["traceEvents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["name"] == "installation.file.verify")
            .unwrap();
        assert_eq!(event["args"]["hash_mode"], expected_mode);
        assert_eq!(event["args"]["configured_workers"], 4);
    }
}

#[test]
fn work_queue_verifies_while_source_job_waits_for_memory_and_drains_only_dispatched_work() {
    let memory = Arc::new(crate::runtime::MemoryBudget::new(2 * 1024 * 1024));
    let occupied = memory.try_acquire(2 * 1024 * 1024).unwrap().unwrap();
    let jobs = vec![ReconstructJob::CopySourceFile {
        file_index: 0,
        source_path: "unused".into(),
    }];
    let mut queue = ReconstructionWorkQueue::default();
    queue
        .ready
        .push_back((ReconstructWorkItem::Job(0), ProfileInstant::detailed(None)));
    queue.ready.push_back((
        ReconstructWorkItem::VerifyFile(1),
        ProfileInstant::detailed(None),
    ));
    let verification = queue.admit_next(&jobs, &memory).unwrap().unwrap();
    assert!(matches!(
        verification.work,
        ReconstructWorkItem::VerifyFile(1)
    ));
    assert_eq!(queue.in_flight, 1);
    assert!(queue.admit_next(&jobs, &memory).unwrap().is_none());
    drop(occupied);
    let source = queue.admit_next(&jobs, &memory).unwrap().unwrap();
    assert!(matches!(source.work, ReconstructWorkItem::Job(0)));
    assert_eq!(queue.in_flight, 2);
    queue.ready.push_back((
        ReconstructWorkItem::VerifyFile(2),
        ProfileInstant::detailed(None),
    ));
    queue.ready.clear();
    assert_eq!(
        queue.in_flight, 2,
        "discarding queued work must not hide dispatched work"
    );
    drop(verification);
    queue.complete();
    drop(source);
    queue.complete();
    assert_eq!(queue.in_flight, 0);
    assert!(memory.try_acquire(2 * 1024 * 1024).unwrap().is_some());
}

fn test_hash(bytes: &[u8]) -> String {
    hash_hex(&blake3::hash(bytes))
}

fn snapshot_manifest_with_file(snapshot_id: &str, full_file_hash: &str) -> SnapshotManifest {
    snapshot_manifest_with_chunk(snapshot_id, full_file_hash, test_hash(b"game"))
}

fn snapshot_manifest_with_chunk(
    snapshot_id: &str,
    full_file_hash: &str,
    chunk_hash: String,
) -> SnapshotManifest {
    let mut manifest = SnapshotManifest::new(snapshot_id.to_string(), SnapshotTagSet::default());
    manifest.push_file(FileManifest {
        path: "game.exe".to_string(),
        size: 4,
        executable: true,
        read_only: false,
        full_file_hash: full_file_hash.to_string(),
        chunks: vec![ChunkRef {
            chunk_hash,
            raw_size: 4,
        }],
    });
    manifest
}

fn bundled_chunk_index() -> crate::repo::index::ChunkIndex {
    crate::repo::index::ChunkIndex {
        format_version: "1".to_string(),

        entries: crate::test_util::chunk_entries(std::collections::BTreeMap::from([(
            test_hash(b"game"),
            crate::repo::index::ChunkIndexEntry::new(
                0,
                0,
                ("bundle-a".to_string()).into(),
                4,
                4,
                manifest::ArtifactStorageMode::Bundled,
            ),
        )])),
    }
}

fn snapshot_manifest_with_two_chunks(
    snapshot_id: &str,
    full_file_hash: &str,
    first_chunk_hash: String,
    second_chunk_hash: String,
) -> SnapshotManifest {
    let mut manifest = SnapshotManifest::new(snapshot_id.to_string(), SnapshotTagSet::default());
    manifest.push_file(FileManifest {
        path: "game.exe".to_string(),
        size: 8,
        executable: true,
        read_only: false,
        full_file_hash: full_file_hash.to_string(),
        chunks: vec![
            ChunkRef {
                chunk_hash: first_chunk_hash,
                raw_size: 4,
            },
            ChunkRef {
                chunk_hash: second_chunk_hash,
                raw_size: 4,
            },
        ],
    });
    manifest
}

#[test]
fn pipeline_plan_copies_identical_source_file_without_bundle_dependency() {
    let source_root = crate::test_util::temp_path("gutch_pipeline_copy_source");
    let destination = crate::test_util::temp_path("gutch_pipeline_copy_destination");
    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&destination);
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::write(source_root.join("game.exe"), b"game").unwrap();
    let source_manifest = snapshot_manifest_with_file("source", &test_hash(b"game"));
    let snapshot_manifest = snapshot_manifest_with_file("snapshot", &test_hash(b"game"));
    let repo_root = crate::test_util::temp_path("gutch_pipeline_copy_repo");
    let _ = std::fs::remove_dir_all(&repo_root);
    let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(&repo_root);

    let plan = build_reconstruct_pipeline_plan(
        &snapshot_manifest,
        &bundled_chunk_index(),
        &repo_paths,
        &HashSet::new(),
        Some(SnapshotReconstructSource {
            snapshot_manifest: &source_manifest,
            destination_dir: &source_root,
        }),
    )
    .unwrap();

    assert_eq!(plan.required_bundle_count, 0);
    assert_eq!(plan.reused_source_chunk_count, 1);
    assert!(matches!(
        plan.jobs[0],
        ReconstructJob::CopySourceFile { .. }
    ));

    apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            publication: super::super::publish::Publication::ReplaceDirectory,
            repo_root: &repo_root,
            compression_profile: crate::manifest::CompressionProfile::default(),
            snapshot_manifest: &snapshot_manifest,
            destination_dir: &destination,
            staging_dir: None,
            source_destination_dir: Some(source_root.clone()),
            runtime_hints: &RuntimeResourceHints::default(),
            existing_artifacts: &HashSet::new(),
            pending_artifact_downloads: &[],
            transfer_policy: crate::transfer::types::TransferConcurrencyPolicy::default(),
            control: &crate::progress::GutchOperationControl::default(),
            download_client: None,
            transfer_progress: None,
        },
        plan,
    )
    .unwrap();
    assert_eq!(
        std::fs::read(destination.join("game.exe")).unwrap(),
        b"game"
    );

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[test]
fn pipeline_plan_reconstructs_changed_source_file_from_bundles() {
    let source_root = crate::test_util::temp_path("gutch_pipeline_rebuild_source");
    let _ = std::fs::remove_dir_all(&source_root);
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::write(source_root.join("game.exe"), b"old!").unwrap();
    let source_manifest =
        snapshot_manifest_with_chunk("source", &test_hash(b"old!"), test_hash(b"old!"));
    let snapshot_manifest =
        snapshot_manifest_with_chunk("snapshot", &test_hash(b"game"), test_hash(b"game"));
    let repo_root = crate::test_util::temp_path("gutch_pipeline_rebuild_repo");
    let _ = std::fs::remove_dir_all(&repo_root);
    let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(&repo_root);

    let plan = build_reconstruct_pipeline_plan(
        &snapshot_manifest,
        &bundled_chunk_index(),
        &repo_paths,
        &HashSet::new(),
        Some(SnapshotReconstructSource {
            snapshot_manifest: &source_manifest,
            destination_dir: &source_root,
        }),
    )
    .unwrap();

    assert_eq!(plan.required_bundle_count, 1);
    assert_eq!(plan.reused_source_chunk_count, 0);
    assert!(matches!(plan.jobs[0], ReconstructJob::Artifact(_)));

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn pipeline_plan_mixes_source_and_bundle_chunks_for_changed_file() {
    let source_root = crate::test_util::temp_path("gutch_pipeline_mixed_source");
    let _ = std::fs::remove_dir_all(&source_root);
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::write(source_root.join("game.exe"), b"gameold!").unwrap();

    let shared_chunk_hash = test_hash(b"game");
    let source_only_chunk_hash = test_hash(b"old!");
    let snapshot_only_chunk_hash = test_hash(b"new!");
    let source_manifest = snapshot_manifest_with_two_chunks(
        "source",
        &test_hash(b"gameold!"),
        shared_chunk_hash.clone(),
        source_only_chunk_hash,
    );
    let snapshot_manifest = snapshot_manifest_with_two_chunks(
        "snapshot",
        &test_hash(b"gamenew!"),
        shared_chunk_hash.clone(),
        snapshot_only_chunk_hash.clone(),
    );
    let repo_root = crate::test_util::temp_path("gutch_pipeline_mixed_repo");
    let _ = std::fs::remove_dir_all(&repo_root);
    let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(&repo_root);
    let chunk_index = crate::repo::index::ChunkIndex {
        format_version: "1".to_string(),

        entries: crate::test_util::chunk_entries(std::collections::BTreeMap::from([(
            snapshot_only_chunk_hash,
            crate::repo::index::ChunkIndexEntry::new(
                0,
                0,
                ("bundle-b".to_string()).into(),
                4,
                4,
                manifest::ArtifactStorageMode::Bundled,
            ),
        )])),
    };

    let plan = build_reconstruct_pipeline_plan(
        &snapshot_manifest,
        &chunk_index,
        &repo_paths,
        &HashSet::new(),
        Some(SnapshotReconstructSource {
            snapshot_manifest: &source_manifest,
            destination_dir: &source_root,
        }),
    )
    .unwrap();

    assert_eq!(plan.required_bundle_count, 1);
    assert_eq!(plan.reused_source_chunk_count, 1);
    assert!(
        plan.jobs
            .iter()
            .any(|job| matches!(job, ReconstructJob::SourceChunk(_)))
    );
    assert!(
        plan.jobs
            .iter()
            .any(|job| matches!(job, ReconstructJob::Artifact(_)))
    );

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn pipeline_reconstructs_mixed_source_and_loose_chunks_with_shared_executor() {
    let source_root = crate::test_util::temp_path("gutch_pipeline_mixed_loose_source");
    let repo_root = crate::test_util::temp_path("gutch_pipeline_mixed_loose_repo");
    let destination = crate::test_util::temp_path("gutch_pipeline_mixed_loose_destination");
    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_dir_all(&destination);
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::write(source_root.join("game.exe"), b"gameold!").unwrap();
    let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(&repo_root);
    crate::repo::paths::ensure_gutch_repo_layout(&repo_paths, "mixed-loose-test").unwrap();

    let shared_chunk_hash = test_hash(b"game");
    let source_only_chunk_hash = test_hash(b"old!");
    let replacement_chunk_hash = test_hash(b"new!");
    let compression_profile =
        manifest::CompressionProfile::for_algorithm(manifest::CompressionKind::Zstd, Some(3));
    let replacement_artifact = crate::repo::artifacts::ArtifactRef::LooseChunk {
        hash: replacement_chunk_hash.clone(),
    };
    let replacement_path = replacement_artifact.local_path(&repo_paths).unwrap();
    std::fs::create_dir_all(replacement_path.parent().unwrap()).unwrap();
    std::fs::write(
        &replacement_path,
        crate::test_util::loose_bytes(b"new!", &compression_profile),
    )
    .unwrap();

    let source_manifest = snapshot_manifest_with_two_chunks(
        "source",
        &test_hash(b"gameold!"),
        shared_chunk_hash.clone(),
        source_only_chunk_hash,
    );
    let snapshot_manifest = snapshot_manifest_with_two_chunks(
        "snapshot",
        &test_hash(b"gamenew!"),
        shared_chunk_hash.clone(),
        replacement_chunk_hash.clone(),
    );
    let chunk_index = crate::repo::index::ChunkIndex {
        format_version: "1".to_string(),

        entries: crate::test_util::chunk_entries(std::collections::BTreeMap::from([(
            replacement_chunk_hash.clone(),
            crate::repo::index::ChunkIndexEntry::new(
                0,
                0,
                (replacement_chunk_hash.clone()).into(),
                4,
                std::fs::metadata(&replacement_path).unwrap().len() as u32,
                manifest::ArtifactStorageMode::LooseChunks,
            ),
        )])),
    };
    let existing_artifacts = HashSet::from([replacement_artifact]);
    let plan = build_reconstruct_pipeline_plan(
        &snapshot_manifest,
        &chunk_index,
        &repo_paths,
        &existing_artifacts,
        Some(SnapshotReconstructSource {
            snapshot_manifest: &source_manifest,
            destination_dir: &source_root,
        }),
    )
    .unwrap();
    assert_eq!(plan.jobs.len(), 2);
    assert!(
        plan.jobs
            .iter()
            .any(|job| matches!(job, ReconstructJob::SourceChunk(_)))
    );
    assert!(plan.jobs.iter().any(|job| matches!(
        job,
        ReconstructJob::Artifact(ReconstructArtifactJob {
            artifact: crate::repo::artifacts::ArtifactRef::LooseChunk { .. },
            ..
        })
    )));

    apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            publication: super::super::publish::Publication::ReplaceDirectory,
            repo_root: &repo_root,
            compression_profile,
            snapshot_manifest: &snapshot_manifest,
            destination_dir: &destination,
            staging_dir: None,
            source_destination_dir: Some(source_root.clone()),
            runtime_hints: &RuntimeResourceHints {
                jobs: Some(2),
                memory_budget_bytes: std::num::NonZeroU64::new(136 * 1024 * 1024),
            },
            existing_artifacts: &existing_artifacts,
            pending_artifact_downloads: &[],
            transfer_policy: crate::transfer::types::TransferConcurrencyPolicy::default(),
            control: &crate::progress::GutchOperationControl::default(),
            download_client: None,
            transfer_progress: None,
        },
        plan,
    )
    .unwrap();
    assert_eq!(
        std::fs::read(destination.join("game.exe")).unwrap(),
        b"gamenew!"
    );

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[test]
fn pipeline_reconstructs_from_loose_chunk_artifact() {
    let repo_root = crate::test_util::temp_path("gutch_pipeline_loose_repo");
    let destination = crate::test_util::temp_path("gutch_pipeline_loose_destination");
    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_dir_all(&destination);
    let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(&repo_root);
    crate::repo::paths::ensure_gutch_repo_layout(&repo_paths, "loose-test").unwrap();

    let chunk_hash = test_hash(b"game");
    let artifact = crate::repo::artifacts::ArtifactRef::LooseChunk {
        hash: chunk_hash.clone(),
    };
    let chunk_path = artifact.local_path(&repo_paths).unwrap();
    std::fs::create_dir_all(chunk_path.parent().unwrap()).unwrap();
    let compression_profile = crate::manifest::CompressionProfile::for_algorithm(
        crate::manifest::CompressionKind::Zstd,
        Some(3),
    );
    std::fs::write(
        &chunk_path,
        crate::test_util::loose_bytes(b"game", &compression_profile),
    )
    .unwrap();
    let chunk_index = crate::repo::index::ChunkIndex {
        format_version: "1".to_string(),

        entries: crate::test_util::chunk_entries(std::collections::BTreeMap::from([(
            chunk_hash.clone(),
            crate::repo::index::ChunkIndexEntry::new(
                0,
                0,
                (chunk_hash.clone()).into(),
                4,
                std::fs::metadata(&chunk_path).unwrap().len() as u32,
                manifest::ArtifactStorageMode::LooseChunks,
            ),
        )])),
    };
    let snapshot_manifest =
        snapshot_manifest_with_chunk("loose-snapshot", &test_hash(b"game"), chunk_hash);
    let existing_artifacts = HashSet::from([artifact]);
    let plan = build_reconstruct_pipeline_plan(
        &snapshot_manifest,
        &chunk_index,
        &repo_paths,
        &existing_artifacts,
        None,
    )
    .unwrap();

    assert_eq!(plan.required_bundle_count, 0);
    assert_eq!(plan.required_artifact_count, 1);
    let result = apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            publication: super::super::publish::Publication::ReplaceDirectory,
            repo_root: &repo_root,
            compression_profile: crate::manifest::CompressionProfile::for_algorithm(
                crate::manifest::CompressionKind::Zstd,
                Some(3),
            ),
            snapshot_manifest: &snapshot_manifest,
            destination_dir: &destination,
            staging_dir: None,
            source_destination_dir: None,
            runtime_hints: &RuntimeResourceHints::default(),
            existing_artifacts: &existing_artifacts,
            pending_artifact_downloads: &[],
            transfer_policy: crate::transfer::types::TransferConcurrencyPolicy::default(),
            control: &crate::progress::GutchOperationControl::default(),
            download_client: None,
            transfer_progress: None,
        },
        plan,
    )
    .unwrap();

    assert_eq!(result.0.reconstruct_result.required_artifact_count, 1);
    assert_eq!(
        std::fs::read(destination.join("game.exe")).unwrap(),
        b"game"
    );
    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[test]
fn bundled_pipeline_reads_bundle_once_and_writes_shared_chunks_to_ranges() {
    let repo_root = crate::test_util::temp_path("gutch_pipeline_bundled_repo");
    let destination = crate::test_util::temp_path("gutch_pipeline_bundled_destination");
    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_dir_all(&destination);
    let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(&repo_root);
    crate::repo::paths::ensure_gutch_repo_layout(&repo_paths, "bundled-test").unwrap();

    let first = b"1111";
    let second = b"2222";
    let third = b"3333";
    let first_hash = test_hash(first);
    let second_hash = test_hash(second);
    let third_hash = test_hash(third);
    let compression_profile = manifest::CompressionProfile::default();
    let mut builder = crate::artifact::bundle::BundleBuilder::new(compression_profile.clone());
    builder.add_chunk(
        crate::artifact::hash::ChunkHash::from_blake3(crate::artifact::hash::hash_chunk(first)),
        first.to_vec(),
    );
    builder.add_chunk(
        crate::artifact::hash::ChunkHash::from_blake3(crate::artifact::hash::hash_chunk(second)),
        second.to_vec(),
    );
    builder.add_chunk(
        crate::artifact::hash::ChunkHash::from_blake3(crate::artifact::hash::hash_chunk(third)),
        third.to_vec(),
    );
    let (bundle_id, bundle_bytes) = builder.build_with_id().unwrap();
    let bundle_reader = crate::artifact::bundle::BundleReader::open(
        std::io::Cursor::new(bundle_bytes.as_slice()),
        &compression_profile,
    )
    .unwrap();
    let compressed_sizes = bundle_reader
        .entries()
        .iter()
        .map(|entry| (entry.chunk_hash.clone(), entry.compressed_size))
        .collect::<HashMap<_, _>>();
    let bundle = crate::repo::artifacts::ArtifactRef::Bundle {
        id: bundle_id.clone(),
    };
    let bundle_path = bundle.local_path(&repo_paths).unwrap();
    std::fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
    std::fs::write(bundle_path, bundle_bytes).unwrap();

    let mut snapshot_manifest =
        SnapshotManifest::new("bundled-snapshot".to_string(), SnapshotTagSet::default());
    snapshot_manifest.push_file(FileManifest {
        path: "first.bin".to_string(),
        size: 8,
        executable: false,
        read_only: false,
        full_file_hash: test_hash(b"11112222"),
        chunks: vec![
            ChunkRef {
                chunk_hash: first_hash.clone(),
                raw_size: 4,
            },
            ChunkRef {
                chunk_hash: second_hash.clone(),
                raw_size: 4,
            },
        ],
    });
    snapshot_manifest.push_file(FileManifest {
        path: "second.bin".to_string(),
        size: 8,
        executable: false,
        read_only: false,
        full_file_hash: test_hash(b"33331111"),
        chunks: vec![
            ChunkRef {
                chunk_hash: third_hash.clone(),
                raw_size: 4,
            },
            ChunkRef {
                chunk_hash: first_hash.clone(),
                raw_size: 4,
            },
        ],
    });
    let chunk_index = crate::repo::index::ChunkIndex {
        format_version: "1".to_string(),

        entries: crate::test_util::chunk_entries(std::collections::BTreeMap::from([
            (
                first_hash,
                crate::repo::index::ChunkIndexEntry::new(
                    0,
                    0,
                    (bundle_id.clone()).into(),
                    4,
                    compressed_sizes
                        [&crate::artifact::hash::ChunkHash::parse_hex(&test_hash(first)).unwrap()],
                    manifest::ArtifactStorageMode::Bundled,
                ),
            ),
            (
                second_hash,
                crate::repo::index::ChunkIndexEntry::new(
                    0,
                    0,
                    (bundle_id.clone()).into(),
                    4,
                    compressed_sizes
                        [&crate::artifact::hash::ChunkHash::parse_hex(&test_hash(second)).unwrap()],
                    manifest::ArtifactStorageMode::Bundled,
                ),
            ),
            (
                third_hash,
                crate::repo::index::ChunkIndexEntry::new(
                    0,
                    0,
                    bundle_id.into(),
                    4,
                    compressed_sizes
                        [&crate::artifact::hash::ChunkHash::parse_hex(&test_hash(third)).unwrap()],
                    manifest::ArtifactStorageMode::Bundled,
                ),
            ),
        ])),
    };
    let existing_artifacts = HashSet::from([bundle]);
    let plan = build_reconstruct_pipeline_plan(
        &snapshot_manifest,
        &chunk_index,
        &repo_paths,
        &existing_artifacts,
        None,
    )
    .unwrap();
    assert_eq!(plan.jobs.len(), 1);
    assert!(matches!(
        &plan.jobs[0],
        ReconstructJob::Artifact(job)
            if matches!(job.artifact, crate::repo::artifacts::ArtifactRef::Bundle { .. })
                && job.chunks.len() == 3
    ));

    apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            publication: super::super::publish::Publication::ReplaceDirectory,
            repo_root: &repo_root,
            compression_profile,
            snapshot_manifest: &snapshot_manifest,
            destination_dir: &destination,
            staging_dir: None,
            source_destination_dir: None,
            runtime_hints: &RuntimeResourceHints {
                jobs: Some(2),
                memory_budget_bytes: std::num::NonZeroU64::new(136 * 1024 * 1024),
            },
            existing_artifacts: &existing_artifacts,
            pending_artifact_downloads: &[],
            transfer_policy: crate::transfer::types::TransferConcurrencyPolicy::default(),
            control: &crate::progress::GutchOperationControl::default(),
            download_client: None,
            transfer_progress: None,
        },
        plan,
    )
    .unwrap();

    assert_eq!(
        std::fs::read(destination.join("first.bin")).unwrap(),
        b"11112222"
    );
    assert_eq!(
        std::fs::read(destination.join("second.bin")).unwrap(),
        b"33331111"
    );
    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_dir_all(&destination);
}

#[test]
fn pipeline_verifies_ready_file_before_next_artifact_without_verifying_partial_file() {
    #[derive(Debug, Default)]
    struct RecordedProgress(Mutex<Vec<String>>);

    impl crate::progress::GutchProgressSink for RecordedProgress {
        fn emit(&self, event: GutchProgressEvent) {
            if event.phase == GutchProgressPhase::Reconstruct
                && event.message.starts_with("Reconstructing ")
            {
                self.0.lock().unwrap().push(event.message);
            }
        }
    }

    let workspace = tempfile::tempdir().unwrap();
    let repo_root = workspace.path().join("repo");
    let destination = workspace.path().join("output");
    let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(&repo_root);
    crate::repo::paths::ensure_gutch_repo_layout(&repo_paths, "readiness-test").unwrap();
    let compression = manifest::CompressionProfile::default();
    let mut chunk_entries = BTreeMap::new();
    let mut existing_artifacts = HashSet::new();
    let mut chunk_refs = Vec::new();
    for bytes in [b"first".as_slice(), b"second".as_slice()] {
        let hash = crate::artifact::hash::ChunkHash::from_blake3(blake3::hash(bytes));
        let mut builder = crate::artifact::bundle::BundleBuilder::new(compression.clone());
        builder.add_chunk(hash, bytes.to_vec());
        let (bundle_id, bundle_bytes) = builder.build_with_id().unwrap();
        let reader = crate::artifact::bundle::BundleReader::open(
            std::io::Cursor::new(bundle_bytes.as_slice()),
            &compression,
        )
        .unwrap();
        let artifact = crate::repo::artifacts::ArtifactRef::Bundle {
            id: bundle_id.clone(),
        };
        let artifact_path = artifact.local_path(&repo_paths).unwrap();
        std::fs::create_dir_all(artifact_path.parent().unwrap()).unwrap();
        std::fs::write(&artifact_path, &bundle_bytes).unwrap();
        chunk_entries.insert(
            hash.to_hex(),
            crate::repo::index::ChunkIndexEntry::new(
                0,
                0,
                bundle_id.into(),
                bytes.len() as u32,
                reader.entries()[0].compressed_size,
                manifest::ArtifactStorageMode::Bundled,
            ),
        );
        chunk_refs.push(ChunkRef {
            chunk_hash: hash.to_hex(),
            raw_size: bytes.len() as u32,
        });
        existing_artifacts.insert(artifact);
    }
    let chunk_index = crate::repo::index::ChunkIndex {
        format_version: "1".to_string(),
        entries: crate::test_util::chunk_entries(chunk_entries),
    };
    let mut snapshot = SnapshotManifest::new("readiness".to_string(), SnapshotTagSet::default());
    // Bundle one completes the first file, but only supplies the dependent file's
    // prefix. Bundle two must finish before that second file can pass verification.
    for (path, bytes, chunks) in [
        (
            "a-ready.bin",
            b"first".as_slice(),
            vec![chunk_refs[0].clone()],
        ),
        ("b-dependent.bin", b"firstsecond".as_slice(), chunk_refs),
    ] {
        snapshot.push_file(FileManifest {
            path: path.to_string(),
            size: bytes.len() as u64,
            executable: false,
            read_only: false,
            full_file_hash: test_hash(bytes),
            chunks,
        });
    }
    let plan = build_reconstruct_pipeline_plan(
        &snapshot,
        &chunk_index,
        &repo_paths,
        &existing_artifacts,
        None,
    )
    .unwrap();
    assert_eq!(plan.jobs.len(), 2);
    let progress = Arc::new(RecordedProgress::default());
    // Record coordinator events directly: live UI feedback intentionally coalesces them.
    let control = GutchOperationControl::default().with_replaced_progress(progress.clone());
    apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            publication: super::super::publish::Publication::ReplaceDirectory,
            repo_root: &repo_root,
            compression_profile: compression,
            snapshot_manifest: &snapshot,
            destination_dir: &destination,
            staging_dir: None,
            source_destination_dir: None,
            runtime_hints: &RuntimeResourceHints {
                jobs: Some(1),
                ..Default::default()
            },
            existing_artifacts: &existing_artifacts,
            pending_artifact_downloads: &[],
            transfer_policy: crate::transfer::types::TransferConcurrencyPolicy::default(),
            control: &control,
            download_client: None,
            transfer_progress: None,
        },
        plan,
    )
    .unwrap();
    let events = progress.0.lock().unwrap();
    assert_eq!(
        &events[..4],
        [
            "Reconstructing jobs (1/2)",
            "Reconstructing files (1/2)",
            "Reconstructing jobs (2/2)",
            "Reconstructing files (2/2)",
        ],
    );
    assert_eq!(
        std::fs::read(destination.join("a-ready.bin")).unwrap(),
        b"first",
    );
    assert_eq!(
        std::fs::read(destination.join("b-dependent.bin")).unwrap(),
        b"firstsecond",
    );
}

#[test]
fn pipeline_finishes_when_download_workers_exit_before_reconstruction() {
    use std::io::Read;
    use std::io::Write;
    let workspace = tempfile::tempdir().unwrap();
    let repo_root = workspace.path().join("repo");
    let destination = workspace.path().join("output");
    let paths = crate::repo::paths::GutchRepoPaths::from_repo_root(&repo_root);
    let compression = manifest::CompressionProfile::default();
    let chunk_hash = test_hash(b"game");
    let artifact = crate::repo::artifacts::ArtifactRef::LooseChunk {
        hash: chunk_hash.clone(),
    };
    let payload = crate::test_util::loose_bytes(b"game", &compression);
    let compressed_size = payload.len() as u64;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/chunk", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        )
        .unwrap();
        stream.write_all(&payload).unwrap();
    });
    let mut snapshot = snapshot_manifest_with_file("snapshot", &chunk_hash);
    // One download fans out to many output files, keeping reconstruction busy
    // after the download worker has sent its final event and closed the channel.
    for index in 0..256 {
        let mut file = snapshot.files().next().unwrap().clone();
        file.path = format!("copy-{index}.bin");
        snapshot.push_file(file);
    }
    let index = crate::repo::index::ChunkIndex {
        format_version: "1".into(),

        entries: crate::test_util::chunk_entries(BTreeMap::from([(
            chunk_hash.clone(),
            crate::repo::index::ChunkIndexEntry::new(
                0,
                0,
                (chunk_hash.clone()).into(),
                4,
                compressed_size as u32,
                manifest::ArtifactStorageMode::LooseChunks,
            ),
        )])),
    };
    let plan =
        build_reconstruct_pipeline_plan(&snapshot, &index, &paths, &HashSet::new(), None).unwrap();
    let pending = [PendingArtifactDownload {
        artifact_path: artifact.local_path(&paths).unwrap(),
        artifact,
        artifact_id: chunk_hash,
        url,
        ordinal: 1,
        missing_artifact_count: 1,
    }];
    apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            publication: super::super::publish::Publication::ReplaceDirectory,
            repo_root: &repo_root,
            compression_profile: compression,
            snapshot_manifest: &snapshot,
            destination_dir: &destination,
            staging_dir: None,
            source_destination_dir: None,
            runtime_hints: &RuntimeResourceHints {
                jobs: Some(1),
                ..Default::default()
            },
            existing_artifacts: &HashSet::new(),
            pending_artifact_downloads: &pending,
            transfer_policy: crate::transfer::types::TransferConcurrencyPolicy::default(),
            control: &crate::progress::GutchOperationControl::default(),
            download_client: Some(&crate::transfer::http::build_download_client().unwrap()),
            transfer_progress: Some(&Arc::new(FetchTransferProgress::default())),
        },
        plan,
    )
    .unwrap();
    server.join().unwrap();
    for file in snapshot.files() {
        assert_eq!(
            std::fs::read(destination.join(&file.path)).unwrap(),
            b"game"
        );
    }
}
