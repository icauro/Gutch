use super::*;
use crate::artifact::hash::ChunkHash;
use crate::artifact::hash::hash_hex;
use crate::artifact::integrity::hash_file;
use crate::artifact::profiles::ArtifactStorageMode;
use crate::artifact::profiles::BundleSizingPreset;
use crate::artifact::profiles::ChunkSizingPreset;
use crate::artifact::profiles::Chunker;
use crate::artifact::profiles::CompressionKind;
use crate::capture::plan::plan_snapshot_capture;
use crate::capture::scan::format_system_time_utc;
use crate::capture::scan::source_modified_at;
use crate::capture::sources::discovery::source_snapshot_candidate;
use crate::installation::InstallationRequest;
use crate::installation::reconstruct_snapshot;
use crate::manifest;
use crate::manifest::ChunkRef;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::ensure_gutch_repo_layout;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::read_repo_status;
use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
use crate::repo::snapshots::retag_snapshot_by_id;
use crate::repo::verify::RepoArtifactVerification;
use crate::repo::verify::verify_gutch_repo_artifacts;
use crate::runtime::RuntimeResourceHints;
use crate::snapshot::identity::snapshot_id_from_entries;
use std::fs;
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn plan_snapshot_capture_keeps_tagless_snapshots_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    let source_root = temp.path().join("sources");
    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

    let new_source = source_root.join("game-v1.0.0-windows-x86_64-base-2026_07_14");
    fs::create_dir_all(&new_source).unwrap();
    fs::write(new_source.join("game.exe"), b"old-content").unwrap();

    let old_candidate = source_snapshot_candidate(
        &new_source,
        &manifest::SnapshotTagSet::default(),
        String::new(),
    )
    .unwrap();
    let old_files = vec![manifest::FileManifest {
        path: "game.exe".into(),
        size: 11,
        executable: false,
        read_only: false,
        full_file_hash: ChunkHash::from_blake3(
            hash_file(new_source.join("game.exe").as_path()).unwrap(),
        ),
        chunks: vec![ChunkRef {
            chunk_hash: ChunkHash::parse_hex(&hash_hex(
                &hash_file(new_source.join("game.exe").as_path()).unwrap(),
            ))
            .unwrap(),
            raw_size: 11,
        }],
    }];
    let old_entries = old_files
        .iter()
        .cloned()
        .map(Into::into)
        .collect::<Vec<_>>();
    let old_snapshot_id = snapshot_id_from_entries(&old_entries);
    let mut old_manifest =
        SnapshotManifest::new(old_snapshot_id.clone(), old_candidate.snapshot_tags.clone());
    old_manifest.source_modified_at = format_system_time_utc(
        source_modified_at(&new_source, &OperationControl::default()).unwrap(),
    );
    old_manifest.set_files(old_files);
    write_snapshot_manifest_artifacts(
        &repo_paths,
        &old_manifest,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();

    fs::write(new_source.join("game.exe"), b"new-content").unwrap();

    let plan = plan_snapshot_capture(
        &repo_root,
        &new_source,
        &manifest::SnapshotTagSet::default(),
        &OperationControl::default(),
    )
    .unwrap();
    assert_eq!(plan.snapshot_tags, manifest::SnapshotTagSet::default());
}

fn test_tags() -> manifest::SnapshotTagSet {
    manifest::SnapshotTagSet::from_compact_strings(&[
        "version=1.0.0".to_string(),
        "os=windows".to_string(),
    ])
    .unwrap()
}

fn test_capture(
    repo_root: &Path,
    source_dir: &Path,
    tags: &manifest::SnapshotTagSet,
    control: OperationControl,
) -> SnapshotCaptureResult {
    capture_snapshot(SnapshotCaptureRequest {
        repo_root: repo_root.to_path_buf(),
        source_dir: source_dir.to_path_buf(),
        explicit_tags: tags.clone(),
        chunk_profile: ChunkSizingPreset::Chunk128KiB,
        chunker: Chunker::FastCdc,
        bundle_profile: BundleSizingPreset::Bundle8MiB,
        compression_algorithm: CompressionKind::Zstd,
        compression_level: None,
        storage_mode: ArtifactStorageMode::Bundled,
        runtime_hints: RuntimeResourceHints::default(),
        control,
    })
    .unwrap()
}

fn test_repo_with_source(
    temp: &tempfile::TempDir,
    source_name: &str,
    content: &[u8],
) -> (PathBuf, PathBuf) {
    let repo_root = temp.path().join("repo");
    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    ensure_gutch_repo_layout(&repo_paths, "base").unwrap();
    let source_dir = temp.path().join(source_name);
    fs::create_dir_all(&source_dir).unwrap();
    fs::write(source_dir.join("game.exe"), content).unwrap();
    (repo_root, source_dir)
}

fn test_reconstruct_and_strict_verify(repo_root: &Path, snapshot_id: &str) {
    let destination = repo_root.join(format!("reconstructed-{snapshot_id}"));
    reconstruct_snapshot(InstallationRequest {
        repo_path: repo_root.to_path_buf(),
        snapshot_id: snapshot_id.to_string(),
        destination_dir: destination.clone(),
        staging_dir: None,
        source_snapshot_id: None,
        source_dir: None,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    })
    .unwrap();
    assert!(destination.join("game.exe").is_file());
    let verification = verify_gutch_repo_artifacts(
        repo_root,
        Some(snapshot_id),
        RepoArtifactVerification::Strict,
    )
    .unwrap();
    assert!(verification.ok, "strict verification failed");
}

#[test]
fn switching_to_jc_rechunks_unchanged_files_in_a_new_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let mut seed = 917u64;
    let data: Vec<u8> = (0..2_100_000)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect();
    let (repo_root, source) = test_repo_with_source(&temp, "game", &data);
    let first = test_capture(&repo_root, &source, &test_tags(), Default::default());
    // Change another file so content identity creates a new snapshot while
    // game.exe exercises reuse of an unchanged file across chunker settings.
    fs::write(source.join("new.txt"), b"new snapshot").unwrap();
    let second = capture_snapshot(SnapshotCaptureRequest {
        repo_root: repo_root.clone(),
        source_dir: source,
        explicit_tags: test_tags(),
        chunk_profile: ChunkSizingPreset::Chunk128KiB,
        chunker: Chunker::Jc,
        bundle_profile: BundleSizingPreset::Bundle8MiB,
        compression_algorithm: CompressionKind::Zstd,
        compression_level: None,
        storage_mode: ArtifactStorageMode::Bundled,
        runtime_hints: Default::default(),
        control: Default::default(),
    })
    .unwrap();
    let paths = RepoPaths::from_repo_root(&repo_root);
    let old = crate::repo::snapshot_manifests::load_snapshot_manifest(&paths, &first.snapshot_id)
        .unwrap();
    let new = crate::repo::snapshot_manifests::load_snapshot_manifest(&paths, &second.snapshot_id)
        .unwrap();
    assert_eq!(old.chunker, Chunker::FastCdc);
    assert_eq!(new.chunker, Chunker::Jc);
    let old_file = old.files().find(|file| file.path == "game.exe").unwrap();
    let new_file = new.files().find(|file| file.path == "game.exe").unwrap();
    assert_ne!(
        serde_json::to_vec(&old_file.chunks).unwrap(),
        serde_json::to_vec(&new_file.chunks).unwrap()
    );
    test_reconstruct_and_strict_verify(&repo_root, &second.snapshot_id);
    assert_eq!(
        fs::read(
            repo_root
                .join(format!("reconstructed-{}", second.snapshot_id))
                .join("game.exe")
        )
        .unwrap(),
        data
    );
}

#[test]
fn same_tags_create_distinct_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let (repo_root, source_a) = test_repo_with_source(&temp, "game-a", b"content-a");
    let source_b = temp.path().join("game-b");
    fs::create_dir_all(&source_b).unwrap();
    fs::write(source_b.join("game.exe"), b"content-b").unwrap();

    let first = test_capture(
        &repo_root,
        &source_a,
        &test_tags(),
        OperationControl::default(),
    );
    assert!(matches!(
        first.outcome,
        SnapshotCaptureOutcome::Created { .. }
    ));
    let second = test_capture(
        &repo_root,
        &source_b,
        &test_tags(),
        OperationControl::default(),
    );
    assert!(matches!(
        second.outcome,
        SnapshotCaptureOutcome::Created { .. }
    ));
    assert_ne!(first.snapshot_id, second.snapshot_id);

    for snapshot_id in [&first.snapshot_id, &second.snapshot_id] {
        let (repo_paths, _) = load_gutch_repo_manifest(&repo_root).unwrap();
        crate::repo::snapshot_manifests::load_snapshot_manifest(&repo_paths, snapshot_id).unwrap();
        test_reconstruct_and_strict_verify(&repo_root, snapshot_id);
    }
}

#[test]
fn cancelled_capture_retains_first_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let (repo_root, source_a) = test_repo_with_source(&temp, "game-a", b"content-a");
    let source_b = temp.path().join("game-b");
    fs::create_dir_all(&source_b).unwrap();
    fs::write(source_b.join("game.exe"), b"content-b").unwrap();

    let first = test_capture(
        &repo_root,
        &source_a,
        &test_tags(),
        OperationControl::default(),
    );
    let cancelled = OperationControl::default()
        .with_cancellation(Arc::new(crate::test_util::TestCancellation::cancelled()));
    let failed = capture_snapshot(SnapshotCaptureRequest {
        repo_root: repo_root.clone(),
        source_dir: source_b,
        explicit_tags: test_tags(),
        chunk_profile: ChunkSizingPreset::Chunk128KiB,
        chunker: Chunker::FastCdc,
        bundle_profile: BundleSizingPreset::Bundle8MiB,
        compression_algorithm: CompressionKind::Zstd,
        compression_level: None,
        storage_mode: ArtifactStorageMode::Bundled,
        runtime_hints: RuntimeResourceHints::default(),
        control: cancelled,
    });
    assert!(failed.is_err());

    let (repo_paths, _) = load_gutch_repo_manifest(&repo_root).unwrap();
    let retained =
        crate::repo::snapshot_manifests::load_snapshot_manifest(&repo_paths, &first.snapshot_id)
            .unwrap();
    assert_eq!(retained.snapshot_tags, test_tags());
    let status = read_repo_status(&repo_root).unwrap();
    assert_eq!(status.snapshots.len(), 1);
    test_reconstruct_and_strict_verify(&repo_root, &first.snapshot_id);
}

#[test]
fn identical_recapture_returns_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let (repo_root, source) = test_repo_with_source(&temp, "game", b"content");

    let first = test_capture(
        &repo_root,
        &source,
        &test_tags(),
        OperationControl::default(),
    );
    let second = test_capture(
        &repo_root,
        &source,
        &test_tags(),
        OperationControl::default(),
    );
    assert_eq!(first.snapshot_id, second.snapshot_id);
    assert!(matches!(
        second.outcome,
        SnapshotCaptureOutcome::Unchanged { .. }
    ));
    assert_eq!(second.generated_bundle_count, 0);
    assert_eq!(second.generated_loose_chunk_count, 0);

    let status = read_repo_status(&repo_root).unwrap();
    assert_eq!(status.snapshots.len(), 1);
}

#[test]
fn same_id_different_tags_returns_metadata_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let (repo_root, source) = test_repo_with_source(&temp, "game", b"content");

    let first = test_capture(
        &repo_root,
        &source,
        &test_tags(),
        OperationControl::default(),
    );
    let other_tags = manifest::SnapshotTagSet::from_compact_strings(&[
        "version=2.0.0".to_string(),
        "os=windows".to_string(),
    ])
    .unwrap();
    let conflict = capture_snapshot(SnapshotCaptureRequest {
        repo_root: repo_root.clone(),
        source_dir: source,
        explicit_tags: other_tags,
        chunk_profile: ChunkSizingPreset::Chunk128KiB,
        chunker: Chunker::FastCdc,
        bundle_profile: BundleSizingPreset::Bundle8MiB,
        compression_algorithm: CompressionKind::Zstd,
        compression_level: None,
        storage_mode: ArtifactStorageMode::Bundled,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    });
    let Err(crate::Error::SnapshotMetadataConflict {
        snapshot_id,
        differences,
    }) = conflict
    else {
        panic!("expected a metadata conflict, got {conflict:?}");
    };
    assert_eq!(snapshot_id, first.snapshot_id);
    assert!(
        differences
            .iter()
            .any(|difference| difference.field == "snapshot_tags")
    );

    let (repo_paths, _) = load_gutch_repo_manifest(&repo_root).unwrap();
    let stored =
        crate::repo::snapshot_manifests::load_snapshot_manifest(&repo_paths, &first.snapshot_id)
            .unwrap();
    assert_eq!(stored.snapshot_tags, test_tags());
}

#[test]
fn recapture_after_product_metadata_edit_is_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let (repo_root, source) = test_repo_with_source(&temp, "game", b"content");
    let first = test_capture(
        &repo_root,
        &source,
        &test_tags(),
        OperationControl::default(),
    );
    let metadata = manifest::SnapshotProductMetadata {
        document_type: Default::default(),
        document_version: Default::default(),
        runtime: manifest::SnapshotRuntimeMetadata::Direct(
            manifest::SnapshotDirectRuntimeMetadata {
                launch_actions: vec![manifest::SnapshotLaunchAction {
                    action_name: "Play".into(),
                    executable_path: "game.exe".into(),
                    command_arguments: Vec::new(),
                    working_directory_path: None,
                }],
                external_dependencies: Vec::new(),
            },
        ),
    };
    crate::repo::snapshot_manifests::save_snapshot_product_metadata_for_snapshots(
        &repo_root,
        std::slice::from_ref(&first.snapshot_id),
        Some(metadata),
    )
    .unwrap();

    let second = test_capture(
        &repo_root,
        &source,
        &test_tags(),
        OperationControl::default(),
    );
    assert!(matches!(
        second.outcome,
        SnapshotCaptureOutcome::Unchanged { .. }
    ));
}

#[test]
fn recapture_reports_repairs_and_always_emits_done() {
    #[derive(Debug, Default)]
    struct DoneEvents(std::sync::atomic::AtomicBool);
    impl crate::progress::ProgressSink for DoneEvents {
        fn emit(&self, event: crate::progress::ProgressEvent) {
            if event.phase == crate::progress::ProgressPhase::Done
                && event.message == "Snapshot capture complete"
            {
                self.0.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let (repo_root, source) = test_repo_with_source(&temp, "game", b"content");
    let first = test_capture(
        &repo_root,
        &source,
        &test_tags(),
        OperationControl::default(),
    );
    let done = Arc::new(DoneEvents::default());
    // Live feedback may repeat the latest event, so check that one arrived.
    let control = || {
        done.0.store(false, std::sync::atomic::Ordering::Relaxed);
        OperationControl::default().with_progress(done.clone())
    };

    let unchanged = test_capture(&repo_root, &source, &test_tags(), control());
    assert!(matches!(
        unchanged.outcome,
        SnapshotCaptureOutcome::Unchanged { .. }
    ));
    assert!(done.0.load(std::sync::atomic::Ordering::Relaxed));

    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    fs::remove_dir_all(&repo_paths.bundles_root).unwrap();
    fs::create_dir_all(&repo_paths.bundles_root).unwrap();
    let repaired = test_capture(&repo_root, &source, &test_tags(), control());
    assert!(matches!(
        repaired.outcome,
        SnapshotCaptureOutcome::Repaired { .. }
    ));
    assert!(done.0.load(std::sync::atomic::Ordering::Relaxed));
    test_reconstruct_and_strict_verify(&repo_root, &first.snapshot_id);
}

#[test]
fn explicit_retag_reports_previous_and_new_tags() {
    let temp = tempfile::tempdir().unwrap();
    let (repo_root, source) = test_repo_with_source(&temp, "game", b"content");

    let first = test_capture(
        &repo_root,
        &source,
        &test_tags(),
        OperationControl::default(),
    );
    let new_tags = manifest::SnapshotTagSet::from_compact_strings(&[
        "version=2.0.0".to_string(),
        "os=windows".to_string(),
    ])
    .unwrap();
    let retagged = retag_snapshot_by_id(
        &repo_root,
        &first.snapshot_id,
        new_tags.clone(),
        &OperationControl::default(),
    )
    .unwrap();
    assert_eq!(retagged.snapshot_id, first.snapshot_id);
    assert_eq!(retagged.previous_tags, test_tags());
    assert_eq!(retagged.new_tags, new_tags);

    let (repo_paths, _) = load_gutch_repo_manifest(&repo_root).unwrap();
    let stored =
        crate::repo::snapshot_manifests::load_snapshot_manifest(&repo_paths, &first.snapshot_id)
            .unwrap();
    assert_eq!(stored.snapshot_tags, new_tags);
}

#[test]
fn tampered_blueprint_returns_integrity_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let (repo_root, source) = test_repo_with_source(&temp, "game", b"content");

    let first = test_capture(
        &repo_root,
        &source,
        &test_tags(),
        OperationControl::default(),
    );
    let (repo_paths, _) = load_gutch_repo_manifest(&repo_root).unwrap();
    let mut stored =
        crate::repo::snapshot_manifests::load_snapshot_manifest(&repo_paths, &first.snapshot_id)
            .unwrap();
    stored.entries.clear();
    write_snapshot_manifest_artifacts(
        &repo_paths,
        &stored,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();

    let reader_error = crate::repo::read::load_snapshot_manifest(&repo_paths, &first.snapshot_id)
        .expect_err("stored blueprint identity must be checked by repository readers");
    assert!(
        matches!(reader_error, crate::Error::SnapshotIntegrityMismatch { snapshot_id }
        if snapshot_id == first.snapshot_id)
    );

    let conflict = capture_snapshot(SnapshotCaptureRequest {
        repo_root: repo_root.clone(),
        source_dir: source,
        explicit_tags: test_tags(),
        chunk_profile: ChunkSizingPreset::Chunk128KiB,
        chunker: Chunker::FastCdc,
        bundle_profile: BundleSizingPreset::Bundle8MiB,
        compression_algorithm: CompressionKind::Zstd,
        compression_level: None,
        storage_mode: ArtifactStorageMode::Bundled,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    });
    assert!(
        matches!(&conflict, Err(crate::Error::SnapshotIntegrityMismatch { snapshot_id })
        if *snapshot_id == first.snapshot_id),
        "expected an integrity mismatch, got {conflict:?}"
    );
}

#[test]
fn content_change_with_frozen_mtime_creates_new_snapshot() {
    use std::time::SystemTime;

    let temp = tempfile::tempdir().unwrap();
    let (repo_root, source_a) = test_repo_with_source(&temp, "game-a", b"content-a");
    let source_b = temp.path().join("game-b");
    fs::create_dir_all(&source_b).unwrap();
    fs::write(source_b.join("game.exe"), b"content-b").unwrap();

    // Freeze modification times: snapshot identity must follow content,
    // never timestamps.
    let frozen = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for source in [&source_a, &source_b] {
        File::options()
            .write(true)
            .open(source.join("game.exe"))
            .unwrap()
            .set_modified(frozen)
            .unwrap();
    }

    let first = test_capture(
        &repo_root,
        &source_a,
        &test_tags(),
        OperationControl::default(),
    );
    let second = test_capture(
        &repo_root,
        &source_b,
        &test_tags(),
        OperationControl::default(),
    );
    assert_ne!(first.snapshot_id, second.snapshot_id);

    let status = read_repo_status(&repo_root).unwrap();
    assert_eq!(status.snapshots.len(), 2);
}

#[cfg(unix)]
#[test]
fn failed_artifact_write_publishes_no_snapshot() {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let (repo_root, source_a) = test_repo_with_source(&temp, "game-a", b"content-a");
    let source_b = temp.path().join("game-b");
    fs::create_dir_all(&source_b).unwrap();
    fs::write(source_b.join("game.exe"), b"content-b").unwrap();

    let first = test_capture(
        &repo_root,
        &source_a,
        &test_tags(),
        OperationControl::default(),
    );

    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    let read_only = Permissions::from_mode(0o555);
    fs::set_permissions(&repo_paths.bundles_root, read_only.clone()).unwrap();
    fs::set_permissions(&repo_paths.chunks_root, read_only).unwrap();
    let failed = capture_snapshot(SnapshotCaptureRequest {
        repo_root: repo_root.clone(),
        source_dir: source_b,
        explicit_tags: test_tags(),
        chunk_profile: ChunkSizingPreset::Chunk128KiB,
        chunker: Chunker::FastCdc,
        bundle_profile: BundleSizingPreset::Bundle8MiB,
        compression_algorithm: CompressionKind::Zstd,
        compression_level: None,
        storage_mode: ArtifactStorageMode::Bundled,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    });
    let read_write = Permissions::from_mode(0o755);
    fs::set_permissions(&repo_paths.bundles_root, read_write.clone()).unwrap();
    fs::set_permissions(&repo_paths.chunks_root, read_write).unwrap();
    assert!(failed.is_err());

    // No partial snapshot became visible and the first snapshot still
    // verifies and reconstructs.
    let status = read_repo_status(&repo_root).unwrap();
    assert_eq!(status.snapshots.len(), 1);
    assert_eq!(status.snapshots[0].snapshot_id, first.snapshot_id);
    test_reconstruct_and_strict_verify(&repo_root, &first.snapshot_id);
}

#[test]
fn capture_bundle_counts_include_each_referenced_bundle_once() {
    use crate::artifact::profiles::ArtifactStorageMode;
    for storage_mode in [
        ArtifactStorageMode::Bundled,
        ArtifactStorageMode::LooseChunks,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (repo_root, source_dir) = test_repo_with_source(&temp, "source", b"first file");
        let capture = |mode| {
            capture_snapshot(SnapshotCaptureRequest {
                repo_root: repo_root.clone(),
                source_dir: source_dir.clone(),
                explicit_tags: test_tags(),
                chunk_profile: ChunkSizingPreset::Chunk128KiB,
                chunker: Chunker::FastCdc,
                bundle_profile: BundleSizingPreset::Bundle8MiB,
                compression_algorithm: CompressionKind::Zstd,
                compression_level: None,
                storage_mode: mode,
                runtime_hints: RuntimeResourceHints::default(),
                control: Default::default(),
            })
            .unwrap()
        };
        let bundled = usize::from(storage_mode == ArtifactStorageMode::Bundled);
        let first = capture(storage_mode);
        assert_eq!(first.generated_bundle_count, bundled);
        assert_eq!(
            first.required_bundle_count, bundled,
            "fresh capture double-counted new bundles"
        );
        assert_eq!(first.reused_chunk_count, 0);
        let repeated = capture(storage_mode);
        assert_eq!(repeated.snapshot_id, first.snapshot_id);
        assert_eq!(repeated.generated_bundle_count, 0);
        assert_eq!(repeated.required_bundle_count, bundled);
        assert_eq!(repeated.reused_chunk_count, repeated.required_chunk_count);

        fs::write(source_dir.join("extra.dat"), b"second unique file").unwrap();
        let extended = capture(storage_mode);
        assert_eq!(extended.generated_bundle_count, bundled);
        assert_eq!(extended.required_bundle_count, 2 * bundled);
        assert_eq!(extended.reused_chunk_count, 1);
        assert_eq!(extended.new_chunk_count, 1);

        // Historical bundles remain in the repository, but this snapshot no longer references them.
        fs::remove_file(source_dir.join("game.exe")).unwrap();
        fs::write(source_dir.join("extra.dat"), b"third unique file").unwrap();
        let replaced = capture(storage_mode);
        assert_eq!(replaced.generated_bundle_count, bundled);
        assert_eq!(replaced.required_bundle_count, bundled);
        assert_eq!(replaced.reused_chunk_count, 0);
        fs::write(source_dir.join("mixed.dat"), b"fourth unique file").unwrap();
        let other_mode = if storage_mode == ArtifactStorageMode::Bundled {
            ArtifactStorageMode::LooseChunks
        } else {
            ArtifactStorageMode::Bundled
        };
        let mixed = capture(other_mode);
        assert_eq!(mixed.required_bundle_count, 1);
        assert_eq!(mixed.generated_bundle_count, 1 - bundled);
        assert_eq!(mixed.reused_chunk_count, 1);
        assert_eq!(mixed.new_chunk_count, 1);
    }
}

#[test]
fn batch_capture_reuses_chunks_from_earlier_sources_in_the_batch() {
    let temp = tempfile::tempdir().unwrap();
    let (repo_root, first_source) = test_repo_with_source(&temp, "first", b"shared file");
    let second_source = temp.path().join("second");
    fs::create_dir_all(&second_source).unwrap();
    fs::write(second_source.join("game.exe"), b"shared file").unwrap();
    fs::write(second_source.join("extra.dat"), b"second only").unwrap();
    let request = |source_dir: &Path, version: &str| SnapshotCaptureRequest {
        repo_root: repo_root.clone(),
        source_dir: source_dir.to_path_buf(),
        explicit_tags: manifest::SnapshotTagSet::from_compact_strings(&[
            format!("version={version}"),
            "os=windows".to_string(),
        ])
        .unwrap(),
        chunk_profile: ChunkSizingPreset::Chunk128KiB,
        chunker: Chunker::FastCdc,
        bundle_profile: BundleSizingPreset::Bundle8MiB,
        compression_algorithm: CompressionKind::Zstd,
        compression_level: None,
        storage_mode: ArtifactStorageMode::Bundled,
        runtime_hints: RuntimeResourceHints::default(),
        control: OperationControl::default(),
    };

    let results = crate::capture::sources::capture_snapshots(vec![
        request(&first_source, "1.0.0"),
        request(&second_source, "1.1.0"),
    ])
    .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].new_chunk_count, 1);
    assert_eq!(results[1].reused_chunk_count, 1);
    assert_eq!(results[1].new_chunk_count, 1);
    for result in &results {
        test_reconstruct_and_strict_verify(&repo_root, &result.snapshot_id);
    }
}
