use super::*;
use crate::hash::hash_hex;
use crate::manifest::{self, ChunkRef, FileManifest, SnapshotTagSet};

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

fn bundled_chunk_index() -> crate::manifest::ChunkIndex {
    crate::manifest::ChunkIndex {
        format_version: "1".to_string(),
        updated_at: String::new(),
        entries: crate::test_util::chunk_entries(std::collections::BTreeMap::from([(
            test_hash(b"game"),
            manifest::ChunkIndexEntry {
                artifact_size: 0,
                chunk_ordinal: 0,
                bundle_id: ("bundle-a".to_string()).into(),
                raw_size: 4,
                compressed_size: 4,
                storage_mode: manifest::ArtifactStorageMode::Bundled,
            },
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
    let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);

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
        plan.files[0].reconstruction,
        ReconstructPipelineReconstruction::CopyFromSource
    ));

    apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            repo_root: &repo_root,
            compression_profile: crate::manifest::CompressionProfile::default(),
            snapshot_manifest: &snapshot_manifest,
            destination_dir: &destination,
            staging_dir: None,
            source_destination_dir: Some(source_root.clone()),
            runtime_hints: &RuntimeResourceHints::default(),
            existing_artifacts: &HashSet::new(),
            pending_bundle_downloads: &[],
            transfer_policy: crate::snapshot_ops::types::TransferConcurrencyPolicy::default(),
            control: &crate::GutchOperationControl::default(),
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
    let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);

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
    assert!(matches!(
        plan.files[0].reconstruction,
        ReconstructPipelineReconstruction::ReconstructFromParts { .. }
    ));

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
    let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);
    let chunk_index = crate::manifest::ChunkIndex {
        format_version: "1".to_string(),
        updated_at: String::new(),
        entries: crate::test_util::chunk_entries(std::collections::BTreeMap::from([(
            snapshot_only_chunk_hash,
            manifest::ChunkIndexEntry {
                artifact_size: 0,
                chunk_ordinal: 0,
                bundle_id: ("bundle-b".to_string()).into(),
                raw_size: 4,
                compressed_size: 4,
                storage_mode: manifest::ArtifactStorageMode::Bundled,
            },
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
    assert!(matches!(
        &plan.files[0].reconstruction,
        ReconstructPipelineReconstruction::ReconstructFromParts { chunks }
            if matches!(chunks[0], ReconstructPipelineChunkInput::Source { .. })
                && matches!(chunks[1], ReconstructPipelineChunkInput::Artifact(_))
    ));

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
    let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);
    crate::ensure_gutch_repo_layout(&repo_paths, "mixed-loose-test").unwrap();

    let shared_chunk_hash = test_hash(b"game");
    let source_only_chunk_hash = test_hash(b"old!");
    let replacement_chunk_hash = test_hash(b"new!");
    let compression_profile =
        manifest::CompressionProfile::for_algorithm(manifest::CompressionKind::Zstd, Some(3));
    let replacement_artifact = crate::ArtifactRef::LooseChunk {
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
    let chunk_index = manifest::ChunkIndex {
        format_version: "1".to_string(),
        updated_at: String::new(),
        entries: crate::test_util::chunk_entries(std::collections::BTreeMap::from([(
            replacement_chunk_hash.clone(),
            manifest::ChunkIndexEntry {
                artifact_size: 0,
                chunk_ordinal: 0,
                bundle_id: (replacement_chunk_hash.clone()).into(),
                raw_size: 4,
                compressed_size: std::fs::metadata(&replacement_path).unwrap().len() as u32,
                storage_mode: manifest::ArtifactStorageMode::LooseChunks,
            },
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
            artifact: crate::ArtifactRef::LooseChunk { .. },
            ..
        })
    )));

    apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            repo_root: &repo_root,
            compression_profile,
            snapshot_manifest: &snapshot_manifest,
            destination_dir: &destination,
            staging_dir: None,
            source_destination_dir: Some(source_root.clone()),
            runtime_hints: &RuntimeResourceHints { jobs: Some(2) },
            existing_artifacts: &existing_artifacts,
            pending_bundle_downloads: &[],
            transfer_policy: crate::snapshot_ops::types::TransferConcurrencyPolicy::default(),
            control: &crate::GutchOperationControl::default(),
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
    let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);
    crate::ensure_gutch_repo_layout(&repo_paths, "loose-test").unwrap();

    let chunk_hash = test_hash(b"game");
    let artifact = crate::ArtifactRef::LooseChunk {
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
    let chunk_index = crate::manifest::ChunkIndex {
        format_version: "1".to_string(),
        updated_at: String::new(),
        entries: crate::test_util::chunk_entries(std::collections::BTreeMap::from([(
            chunk_hash.clone(),
            manifest::ChunkIndexEntry {
                artifact_size: 0,
                chunk_ordinal: 0,
                bundle_id: (chunk_hash.clone()).into(),
                raw_size: 4,
                compressed_size: std::fs::metadata(&chunk_path).unwrap().len() as u32,
                storage_mode: manifest::ArtifactStorageMode::LooseChunks,
            },
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
            pending_bundle_downloads: &[],
            transfer_policy: crate::snapshot_ops::types::TransferConcurrencyPolicy::default(),
            control: &crate::GutchOperationControl::default(),
            download_client: None,
            transfer_progress: None,
        },
        plan,
    )
    .unwrap();

    assert_eq!(result.reconstruct_result.required_artifact_count, 1);
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
    let repo_paths = crate::GutchRepoPaths::from_repo_root(&repo_root);
    crate::ensure_gutch_repo_layout(&repo_paths, "bundled-test").unwrap();

    let first = b"1111";
    let second = b"2222";
    let third = b"3333";
    let first_hash = test_hash(first);
    let second_hash = test_hash(second);
    let third_hash = test_hash(third);
    let compression_profile = manifest::CompressionProfile::default();
    let mut builder = crate::bundle::BundleBuilder::new(compression_profile.clone());
    builder.add_chunk(
        crate::hash::ChunkHash::from_blake3(crate::hash::hash_chunk(first)),
        first.to_vec(),
    );
    builder.add_chunk(
        crate::hash::ChunkHash::from_blake3(crate::hash::hash_chunk(second)),
        second.to_vec(),
    );
    builder.add_chunk(
        crate::hash::ChunkHash::from_blake3(crate::hash::hash_chunk(third)),
        third.to_vec(),
    );
    let (bundle_id, bundle_bytes) = builder.build_with_id().unwrap();
    let bundle_reader = crate::bundle::BundleReader::open(
        std::io::Cursor::new(bundle_bytes.as_slice()),
        &compression_profile,
    )
    .unwrap();
    let compressed_sizes = bundle_reader
        .entries()
        .iter()
        .map(|entry| (entry.chunk_hash.clone(), entry.compressed_size))
        .collect::<HashMap<_, _>>();
    let bundle = crate::ArtifactRef::Bundle {
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
    let chunk_index = manifest::ChunkIndex {
        format_version: "1".to_string(),
        updated_at: String::new(),
        entries: crate::test_util::chunk_entries(std::collections::BTreeMap::from([
            (
                first_hash,
                manifest::ChunkIndexEntry {
                    artifact_size: 0,
                    chunk_ordinal: 0,
                    bundle_id: (bundle_id.clone()).into(),
                    raw_size: 4,
                    compressed_size: compressed_sizes[&test_hash(first)],
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
            ),
            (
                second_hash,
                manifest::ChunkIndexEntry {
                    artifact_size: 0,
                    chunk_ordinal: 0,
                    bundle_id: (bundle_id.clone()).into(),
                    raw_size: 4,
                    compressed_size: compressed_sizes[&test_hash(second)],
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
            ),
            (
                third_hash,
                manifest::ChunkIndexEntry {
                    artifact_size: 0,
                    chunk_ordinal: 0,
                    bundle_id: bundle_id.into(),
                    raw_size: 4,
                    compressed_size: compressed_sizes[&test_hash(third)],
                    storage_mode: manifest::ArtifactStorageMode::Bundled,
                },
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
            if matches!(job.artifact, crate::ArtifactRef::Bundle { .. })
                && job.chunks.len() == 3
    ));

    apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            repo_root: &repo_root,
            compression_profile,
            snapshot_manifest: &snapshot_manifest,
            destination_dir: &destination,
            staging_dir: None,
            source_destination_dir: None,
            runtime_hints: &RuntimeResourceHints { jobs: Some(2) },
            existing_artifacts: &existing_artifacts,
            pending_bundle_downloads: &[],
            transfer_policy: crate::snapshot_ops::types::TransferConcurrencyPolicy::default(),
            control: &crate::GutchOperationControl::default(),
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
fn pipeline_finishes_when_download_workers_exit_before_reconstruction() {
    use std::io::{Read, Write};
    let workspace = tempfile::tempdir().unwrap();
    let repo_root = workspace.path().join("repo");
    let destination = workspace.path().join("output");
    let paths = crate::GutchRepoPaths::from_repo_root(&repo_root);
    let compression = manifest::CompressionProfile::default();
    let chunk_hash = test_hash(b"game");
    let artifact = crate::ArtifactRef::LooseChunk {
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
    let index = manifest::ChunkIndex {
        format_version: "1".into(),
        updated_at: String::new(),
        entries: crate::test_util::chunk_entries(BTreeMap::from([(
            chunk_hash.clone(),
            manifest::ChunkIndexEntry {
                artifact_size: 0,
                chunk_ordinal: 0,
                bundle_id: (chunk_hash.clone()).into(),
                raw_size: 4,
                compressed_size: compressed_size as u32,
                storage_mode: manifest::ArtifactStorageMode::LooseChunks,
            },
        )])),
    };
    let plan =
        build_reconstruct_pipeline_plan(&snapshot, &index, &paths, &HashSet::new(), None).unwrap();
    let pending = [PendingBundleDownload {
        bundle_path: artifact.local_path(&paths).unwrap(),
        artifact,
        bundle_id: chunk_hash,
        url,
        ordinal: 1,
        total_missing_bundle_count: 1,
    }];
    apply_reconstruct_pipeline(
        ReconstructPipelineApplyRequest {
            repo_root: &repo_root,
            compression_profile: compression,
            snapshot_manifest: &snapshot,
            destination_dir: &destination,
            staging_dir: None,
            source_destination_dir: None,
            runtime_hints: &RuntimeResourceHints { jobs: Some(1) },
            existing_artifacts: &HashSet::new(),
            pending_bundle_downloads: &pending,
            transfer_policy: crate::snapshot_ops::types::TransferConcurrencyPolicy::default(),
            control: &crate::GutchOperationControl::default(),
            download_client: Some(&reqwest::blocking::Client::new()),
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
