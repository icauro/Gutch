use super::*;
use crate::artifact::hash::hash_hex;
use crate::installation::io::verify_reconstructed_output_file;
use crate::installation::jobs::*;
use crate::installation::plan::*;
use crate::installation::types::SnapshotReconstructSource;
use crate::manifest::ChunkRef;
use crate::manifest::FileManifest;
use crate::manifest::SnapshotManifest;
use crate::manifest::SnapshotTagSet;
use crate::progress::{GutchOperationControl, GutchProgressEvent, GutchProgressPhase};
use crate::runtime::RuntimeResourceHints;
use crate::transfer::download::PendingArtifactDownload;
use crate::transfer::progress::FetchTransferProgress;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

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
                0,
                &file_plan,
                &paths,
                &[Mutex::new(None)],
                &control,
                4,
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
            chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&chunk_hash).unwrap(),
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
                4,
                0,
                ("bundle-a".to_string()).into(),
                4,
                crate::artifact::profiles::ArtifactStorageMode::Bundled,
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
                chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&first_chunk_hash).unwrap(),
                raw_size: 4,
            },
            ChunkRef {
                chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&second_chunk_hash)
                    .unwrap(),
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
    assert_eq!(plan.required_artifact_count, 0);
    assert!(plan.required_artifacts.is_empty());
    assert_eq!(plan.reused_source_chunk_count, 1);
    assert!(matches!(
        plan.jobs[0],
        ReconstructJob::CopySourceFile { .. }
    ));

    apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            publication: super::super::publish::Publication::ReplaceDirectory,
            repo_root: &repo_root,
            compression_profile: crate::artifact::profiles::CompressionProfile::default(),
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
                4,
                0,
                ("bundle-b".to_string()).into(),
                4,
                crate::artifact::profiles::ArtifactStorageMode::Bundled,
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
    let compression_profile = crate::artifact::profiles::CompressionProfile::for_algorithm(
        crate::artifact::profiles::CompressionKind::Zstd,
        Some(3),
    );
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
                std::fs::metadata(&replacement_path).unwrap().len() as u32,
                0,
                (replacement_chunk_hash.clone()).into(),
                4,
                crate::artifact::profiles::ArtifactStorageMode::LooseChunks,
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
    assert_eq!(plan.required_bundle_count, 0);
    assert_eq!(plan.required_artifact_count, 1);
    assert_eq!(plan.required_artifacts, existing_artifacts);
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
                memory_budget_bytes: std::num::NonZeroU64::new(137 * 1024 * 1024),
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
    let compression_profile = crate::artifact::profiles::CompressionProfile::for_algorithm(
        crate::artifact::profiles::CompressionKind::Zstd,
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
                std::fs::metadata(&chunk_path).unwrap().len() as u32,
                0,
                (chunk_hash.clone()).into(),
                4,
                crate::artifact::profiles::ArtifactStorageMode::LooseChunks,
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
    assert_eq!(plan.required_artifacts, existing_artifacts);
    let result = apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            publication: super::super::publish::Publication::ReplaceDirectory,
            repo_root: &repo_root,
            compression_profile: crate::artifact::profiles::CompressionProfile::for_algorithm(
                crate::artifact::profiles::CompressionKind::Zstd,
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
    let compression_profile = crate::artifact::profiles::CompressionProfile::default();
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
    let bundle_size = bundle_bytes.len() as u32;
    let bundle = crate::repo::artifacts::ArtifactRef::Bundle {
        id: bundle_id.clone().into(),
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
                chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&first_hash).unwrap(),
                raw_size: 4,
            },
            ChunkRef {
                chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&second_hash).unwrap(),
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
                chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&third_hash).unwrap(),
                raw_size: 4,
            },
            ChunkRef {
                chunk_hash: crate::artifact::hash::ChunkHash::parse_hex(&first_hash).unwrap(),
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
                    bundle_size,
                    0,
                    (bundle_id.clone()).into(),
                    4,
                    crate::artifact::profiles::ArtifactStorageMode::Bundled,
                ),
            ),
            (
                second_hash,
                crate::repo::index::ChunkIndexEntry::new(
                    bundle_size,
                    0,
                    (bundle_id.clone()).into(),
                    4,
                    crate::artifact::profiles::ArtifactStorageMode::Bundled,
                ),
            ),
            (
                third_hash,
                crate::repo::index::ChunkIndexEntry::new(
                    bundle_size,
                    0,
                    bundle_id.into(),
                    4,
                    crate::artifact::profiles::ArtifactStorageMode::Bundled,
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
                memory_budget_bytes: std::num::NonZeroU64::new(137 * 1024 * 1024),
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
    let compression = crate::artifact::profiles::CompressionProfile::default();
    let mut chunk_entries = BTreeMap::new();
    let mut existing_artifacts = HashSet::new();
    let mut chunk_refs = Vec::new();
    for bytes in [b"first".as_slice(), b"second".as_slice()] {
        let hash = crate::artifact::hash::ChunkHash::from_blake3(blake3::hash(bytes));
        let mut builder = crate::artifact::bundle::BundleBuilder::new(compression.clone());
        builder.add_chunk(hash, bytes.to_vec());
        let (bundle_id, bundle_bytes) = builder.build_with_id().unwrap();
        let artifact = crate::repo::artifacts::ArtifactRef::Bundle {
            id: bundle_id.clone().into(),
        };
        let artifact_path = artifact.local_path(&repo_paths).unwrap();
        std::fs::create_dir_all(artifact_path.parent().unwrap()).unwrap();
        std::fs::write(&artifact_path, &bundle_bytes).unwrap();
        chunk_entries.insert(
            hash.to_hex(),
            crate::repo::index::ChunkIndexEntry::new(
                bundle_bytes.len() as u32,
                0,
                bundle_id.into(),
                bytes.len() as u32,
                crate::artifact::profiles::ArtifactStorageMode::Bundled,
            ),
        );
        chunk_refs.push(ChunkRef {
            chunk_hash: hash,
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
    let compression = crate::artifact::profiles::CompressionProfile::default();
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
        let request_bytes = stream.read(&mut request).unwrap();
        assert!(request_bytes > 0);
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
                compressed_size as u32,
                0,
                (chunk_hash.clone()).into(),
                4,
                crate::artifact::profiles::ArtifactStorageMode::LooseChunks,
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

#[test]
fn artifact_hash_forwarding_requires_complete_file_and_successful_artifact() {
    use crate::artifact::hash::ChunkHash;
    use crate::installation::handles::{OutputHandleCache, SourceHandleCache};
    use crate::installation::io::{ReconstructJobRequest, reconstruct_job};

    let bytes = b"game";
    let hash = ChunkHash::from_blake3(blake3::hash(bytes));
    let profile = crate::artifact::profiles::CompressionProfile::default();
    for bundled in [false, true] {
        for (corrupt, fail_output, wrong_size) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            if bundled && wrong_size {
                continue;
            }
            let directory = tempfile::tempdir().unwrap();
            let artifact_path = directory.path().join("artifact");
            let (artifact, mut encoded) = if bundled {
                let mut builder = crate::artifact::bundle::BundleBuilder::new(profile.clone());
                builder.add_chunk(hash, bytes.to_vec());
                let (id, encoded) = builder.build_with_id().unwrap();
                (
                    crate::repo::artifacts::ArtifactRef::Bundle { id: id.into() },
                    encoded,
                )
            } else {
                let mut encoded = Vec::new();
                crate::artifact::loose::write_loose_chunk(&mut encoded, hash, bytes, &profile)
                    .unwrap();
                (
                    crate::repo::artifacts::ArtifactRef::LooseChunk {
                        hash: hash.to_hex(),
                    },
                    encoded,
                )
            };
            if corrupt {
                *encoded.last_mut().unwrap() ^= 1;
            }
            std::fs::write(&artifact_path, &encoded).unwrap();
            // Shared whole-file targets, a prefix, a suffix, and a differing full hash.
            let files = [
                (4, hash),
                (4, hash),
                (8, hash),
                (8, hash),
                (4, ChunkHash::from_blake3(blake3::hash(b"else"))),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (size, full_file_hash))| ReconstructPipelineFile {
                path: format!("file-{index}"),
                size,
                full_file_hash,
            })
            .collect::<Vec<_>>();
            let paths = Arc::new(
                files
                    .iter()
                    .map(|file| directory.path().join(&file.path))
                    .collect::<Vec<_>>(),
            );
            if fail_output {
                // The final target stays buffered until artifact completion.
                std::fs::create_dir(&paths[0]).unwrap();
            }
            let sizes = Arc::new(files.iter().map(|file| file.size).collect::<Vec<_>>());
            let handles = OutputHandleCache::new(paths.clone(), sizes, &[1; 5], 4);
            let hashes = (0..files.len())
                .map(|_| Mutex::new(None))
                .collect::<Vec<_>>();
            let target_count = if fail_output { 1 } else { 5 };
            let job = ReconstructJob::Artifact(ReconstructArtifactJob {
                artifact,
                encoded_size: encoded.len() as u32 + u32::from(wrong_size),
                artifact_path,
                chunks: vec![ReconstructArtifactChunk {
                    chunk_hash: hash,
                    raw_size: 4,
                    target_range: 0..target_count,
                }],
                targets: (0..target_count)
                    .map(|file_index| ReconstructTarget {
                        file_index,
                        file_offset: if file_index == 3 { 4 } else { 0 },
                    })
                    .collect(),
            });
            let control = GutchOperationControl::default();
            let result = reconstruct_job(ReconstructJobRequest {
                job: &job,
                file_plans: &files,
                output_paths: &paths,
                precomputed_file_hashes: &hashes,
                output_handles: &handles,
                source_handles: &SourceHandleCache::default(),
                compression_profile: &profile,
                control: &control,
            });
            assert_eq!(
                result.is_err(),
                corrupt || fail_output || wrong_size,
                "bundled={bundled}, corrupt={corrupt}, fail_output={fail_output}, wrong_size={wrong_size}"
            );
            let forwarded = hashes
                .iter()
                .map(|slot| *slot.lock().unwrap())
                .collect::<Vec<_>>();
            if corrupt || fail_output || wrong_size {
                assert_eq!(forwarded, vec![None; 5]);
            } else {
                assert_eq!(forwarded, vec![Some(hash), Some(hash), None, None, None]);
                for index in [0, 1] {
                    assert_eq!(std::fs::read(&paths[index]).unwrap(), bytes);
                    verify_reconstructed_output_file(
                        index,
                        &files[index],
                        &paths,
                        &hashes,
                        &control,
                        4,
                    )
                    .unwrap();
                }
                // A forwarded hash never bypasses the final size check.
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&paths[0])
                    .unwrap()
                    .set_len(3)
                    .unwrap();
                assert!(
                    verify_reconstructed_output_file(0, &files[0], &paths, &hashes, &control, 4)
                        .is_err()
                );
            }
        }
    }
}

#[test]
fn pipeline_plan_preserves_fanout_targets_between_source_and_artifact_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let repo_paths = crate::repo::paths::GutchRepoPaths::from_repo_root(directory.path());
    let chunk_hash =
        |bytes: &[u8]| crate::artifact::hash::ChunkHash::from_blake3(blake3::hash(bytes));
    let make_file = |path: &str, pieces: &[&[u8]]| FileManifest {
        path: path.into(),
        size: pieces.iter().map(|piece| piece.len() as u64).sum(),
        executable: false,
        read_only: false,
        full_file_hash: test_hash(&pieces.concat()),
        chunks: pieces
            .iter()
            .map(|piece| ChunkRef {
                chunk_hash: chunk_hash(piece),
                raw_size: piece.len() as u32,
            })
            .collect(),
    };
    let copied = make_file("copied.bin", &[b"skip", b"copy", b"next", b"tail"]);
    let copied_later = make_file("copied-later.bin", &[b"late"]);
    let mut source = SnapshotManifest::new(test_hash(b"source"), SnapshotTagSet::default());
    source.push_file(copied.clone());
    source.push_file(copied_later.clone());
    source.push_file(make_file("unused.bin", &[b"copy".as_slice(); 16]));
    let mut snapshot = SnapshotManifest::new(test_hash(b"target"), SnapshotTagSet::default());
    snapshot.push_file(copied);
    // Changed files reuse middle chunks from the file already planned as a whole copy.
    snapshot.push_file(make_file(
        "mixed.bin",
        &[b"copy", b"newA", b"newB", b"copy", b"next", b"newA"],
    ));
    snapshot.push_file(make_file(
        "following.bin",
        &[b"copy", b"newB", b"copy", b"next"],
    ));
    snapshot.push_file(copied_later);
    snapshot.push_file(make_file("last.bin", &[b"newC", b"newA", b"newC", b"newB"]));
    let first_bundle = test_hash(b"first bundle");
    let second_bundle = test_hash(b"second bundle");
    let index = crate::repo::index::ChunkIndex {
        format_version: crate::artifact::format::FORMAT_VERSION.into(),
        entries: [
            (b"newA", &first_bundle, 0),
            (b"newB", &first_bundle, 1),
            (b"newC", &second_bundle, 0),
        ]
        .into_iter()
        .map(|(bytes, bundle, ordinal)| {
            (
                chunk_hash(bytes),
                crate::repo::index::ChunkIndexEntry::new(
                    128,
                    ordinal,
                    bundle.as_str().into(),
                    4,
                    crate::artifact::profiles::ArtifactStorageMode::Bundled,
                ),
            )
        })
        .collect(),
    };
    for first_bundle_cached in [false, true] {
        let existing = if first_bundle_cached {
            HashSet::from([crate::repo::artifacts::ArtifactRef::Bundle {
                id: first_bundle.clone().into(),
            }])
        } else {
            HashSet::new()
        };
        let plan = build_reconstruct_pipeline_plan(
            &snapshot,
            &index,
            &repo_paths,
            &existing,
            Some(SnapshotReconstructSource {
                snapshot_manifest: &source,
                destination_dir: directory.path(),
            }),
        )
        .unwrap();
        assert_eq!(plan.required_bundle_count, 2);
        assert_eq!(plan.required_artifact_count, 2);
        let artifact_ids = plan
            .required_artifacts
            .iter()
            .map(|artifact| artifact.id())
            .collect::<HashSet<_>>();
        assert_eq!(
            artifact_ids,
            HashSet::from([first_bundle.as_str(), second_bundle.as_str()])
        );
        assert_eq!(plan.reused_source_chunk_count, 11);
        let first_artifact = crate::repo::artifacts::ArtifactRef::Bundle {
            id: first_bundle.as_str().into(),
        };
        let second_artifact = crate::repo::artifacts::ArtifactRef::Bundle {
            id: second_bundle.as_str().into(),
        };
        if first_bundle_cached {
            assert!(
                !plan
                    .missing_artifact_to_file_indices
                    .contains_key(&first_artifact)
            );
        } else {
            assert_eq!(
                plan.missing_artifact_to_file_indices[&first_artifact],
                [1, 2, 4]
            );
        }
        assert_eq!(plan.missing_artifact_to_file_indices[&second_artifact], [4]);
        assert_eq!(
            plan.missing_artifact_counts,
            if first_bundle_cached {
                vec![0, 0, 0, 0, 1]
            } else {
                vec![0, 1, 1, 0, 2]
            }
        );
        assert_eq!(
            plan.jobs
                .iter()
                .filter(|job| matches!(job, ReconstructJob::CopySourceFile { .. }))
                .count(),
            2
        );
        assert_eq!(
            plan.jobs
                .iter()
                .filter(|job| matches!(job, ReconstructJob::SourceChunk(_)))
                .count(),
            2
        );
        let source_job = plan
            .jobs
            .iter()
            .find_map(|job| match job {
                ReconstructJob::SourceChunk(job) if job.chunk_hash == chunk_hash(b"copy") => {
                    Some(job)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            source_job.source_path.as_ref(),
            &directory.path().join("copied.bin")
        );
        assert_eq!(source_job.source_offset, 4);
        let next_source_job = plan
            .jobs
            .iter()
            .find_map(|job| match job {
                ReconstructJob::SourceChunk(job) if job.chunk_hash == chunk_hash(b"next") => {
                    Some(job)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(next_source_job.source_offset, 8);
        assert_eq!(
            next_source_job
                .targets()
                .map(|target| (target.file_index, target.file_offset))
                .collect::<Vec<_>>(),
            [(1, 16), (2, 12)]
        );
        assert!(Arc::ptr_eq(
            &source_job.source_path,
            &next_source_job.source_path
        ));
        assert_eq!(source_job.raw_size, 4);
        assert_eq!(
            source_job
                .targets()
                .map(|target| (target.file_index, target.file_offset))
                .collect::<Vec<_>>(),
            [(1, 0), (1, 12), (2, 0), (2, 8)]
        );
        let mut targets = BTreeMap::new();
        for job in &plan.jobs {
            if let ReconstructJob::Artifact(job) = job {
                for chunk in &job.chunks {
                    let positions = job.targets[chunk.target_range.clone()]
                        .iter()
                        .map(|target| (target.file_index, target.file_offset))
                        .collect::<Vec<_>>();
                    assert!(targets.insert(chunk.chunk_hash, positions).is_none());
                }
            }
        }
        assert_eq!(
            targets,
            BTreeMap::from([
                (chunk_hash(b"newA"), vec![(1, 4), (1, 20), (4, 4)]),
                (chunk_hash(b"newB"), vec![(1, 8), (2, 4), (4, 12)]),
                (chunk_hash(b"newC"), vec![(4, 0), (4, 8)]),
            ])
        );
        let mut invalid = snapshot.clone();
        let mut invalid_files = invalid.files().cloned().collect::<Vec<_>>();
        invalid_files[2].chunks[0].raw_size = 3;
        invalid_files[2].size -= 1;
        invalid.set_files(invalid_files);
        assert!(
            build_reconstruct_pipeline_plan(
                &invalid,
                &index,
                &repo_paths,
                &existing,
                Some(SnapshotReconstructSource {
                    snapshot_manifest: &source,
                    destination_dir: directory.path()
                }),
            )
            .is_err()
        );
    }
}

#[test]
fn repeated_chunk_planning_preserves_targets_dependencies_and_size_checks() {
    use crate::artifact::hash::ChunkHash;
    use crate::artifact::profiles::ArtifactStorageMode;
    use crate::repo::artifacts::ArtifactRef;
    let workspace = tempfile::tempdir().unwrap();
    let paths = crate::repo::paths::GutchRepoPaths::from_repo_root(workspace.path());
    let hash = ChunkHash::from_blake3(blake3::hash(b"game"));
    for mode in [
        ArtifactStorageMode::Bundled,
        ArtifactStorageMode::LooseChunks,
    ] {
        let id = match mode {
            ArtifactStorageMode::Bundled => "ab".repeat(32),
            ArtifactStorageMode::LooseChunks => hash.to_hex(),
        };
        let entry = crate::repo::index::ChunkIndexEntry::new(128, 0, id.into(), 4, mode);
        let artifact = ArtifactRef::from_chunk_entry(&hash, &entry);
        let index = crate::repo::index::ChunkIndex {
            format_version: "1".into(),
            entries: [(hash, entry)].into_iter().collect(),
        };
        let mut snapshot = snapshot_manifest_with_file("snapshot", &hash.to_hex());
        let mut first = snapshot.files().next().unwrap().clone();
        first.path = "first.bin".into();
        first.chunks.push(first.chunks[0].clone());
        first.size = 8;
        first.full_file_hash = test_hash(b"gamegame");
        let mut second = first.clone();
        second.path = "second.bin".into();
        second.chunks.truncate(1);
        second.size = 4;
        second.full_file_hash = hash.to_hex();
        snapshot.set_files(vec![first, second]);
        for cached in [false, true] {
            let available = if cached {
                HashSet::from([artifact.clone()])
            } else {
                HashSet::new()
            };
            let plan = build_reconstruct_pipeline_plan(&snapshot, &index, &paths, &available, None)
                .unwrap();
            assert_eq!(plan.jobs.len(), 1);
            let ReconstructJob::Artifact(job) = &plan.jobs[0] else {
                panic!("expected artifact job")
            };
            assert_eq!(job.chunks.len(), 1);
            let targets = job.targets[job.chunks[0].target_range.clone()]
                .iter()
                .map(|target| (target.file_index, target.file_offset))
                .collect::<Vec<_>>();
            assert_eq!(targets, [(0, 0), (0, 4), (1, 0)]);
            assert_eq!(
                plan.missing_artifact_counts,
                if cached { vec![0, 0] } else { vec![1, 1] }
            );
            if cached {
                assert!(plan.missing_artifact_to_file_indices.is_empty());
            } else {
                assert_eq!(plan.missing_artifact_to_file_indices[&artifact], [0, 1]);
            }
        }
        // Reject wrong sizes before and after the same hash has been resolved.
        for (file_index, chunk_index) in [(0, 0), (0, 1), (1, 0)] {
            let mut invalid = snapshot.clone();
            let mut files = invalid.files().cloned().collect::<Vec<_>>();
            files[file_index].chunks[chunk_index].raw_size = 3;
            files[file_index].size -= 1;
            invalid.set_files(files);
            let error =
                build_reconstruct_pipeline_plan(&invalid, &index, &paths, &HashSet::new(), None)
                    .err()
                    .expect("wrong chunk size must be rejected");
            assert!(error.to_string().contains("manifest 3, index 4"), "{error}");
        }
    }
}
