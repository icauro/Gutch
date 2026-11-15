use super::super::discovery::discover_source_snapshots;
use super::*;
use crate::capture::scan::format_system_time_utc;
use crate::manifest;
use crate::manifest::snapshot::SnapshotManifest;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::ensure_gutch_repo_layout;
use crate::repo::paths::load_gutch_repo_manifest;
use crate::repo::snapshot_manifests::write_snapshot_manifest_artifacts;
use crate::test_util::{test_chunk_hash, test_hash};
use std::time::Duration;
use std::time::SystemTime;

#[test]
fn plan_snapshot_sources_capture_returns_empty_plan_without_sources_path() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

    let plan =
        plan_snapshot_sources_capture(&repo_root, None, &OperationControl::default()).unwrap();
    assert!(!plan.sources_ready);
    assert_eq!(plan.capturable_count, 0);
    assert!(plan.to_capture.is_empty());
}

#[test]
fn plan_snapshot_sources_capture_keeps_tagless_sources_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    let source_root = temp.path().join("sources");
    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

    let source_dir = source_root.join("game-v2.0.0-windows-x86_64-base-2026_07_14");
    std::fs::create_dir_all(&source_dir).unwrap();
    let game_path = source_dir.join("game.exe");
    std::fs::write(&game_path, b"same-content").unwrap();

    let candidate =
        discover_source_snapshots(&repo_root, &source_root, &OperationControl::default()).unwrap();
    assert_eq!(candidate.len(), 1);

    let tags = candidate[0].snapshot_tags.clone();
    let snapshot_id = test_hash("existing-snapshot-id");
    let mut manifest = SnapshotManifest::new(snapshot_id.clone(), tags);
    manifest.source_modified_at =
        format_system_time_utc(SystemTime::now() - Duration::from_secs(3600));
    manifest.set_files(vec![manifest::FileManifest {
        path: "game.exe".into(),
        size: 12,
        executable: false,
        read_only: false,
        full_file_hash: test_chunk_hash("game.exe"),
        chunks: vec![crate::manifest::ChunkRef {
            chunk_hash: test_chunk_hash("game.exe"),
            raw_size: 12,
        }],
    }]);
    manifest.snapshot_id = crate::snapshot::identity::snapshot_id_from_entries(&manifest.entries);
    write_snapshot_manifest_artifacts(
        &repo_paths,
        &manifest,
        &crate::progress::OperationControl::default(),
    )
    .unwrap();
    crate::test_util::index_manifest(&repo_paths, &manifest);

    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(&game_path, b"new-content").unwrap();

    let plan =
        plan_snapshot_sources_capture(&repo_root, Some(&source_root), &OperationControl::default())
            .unwrap();
    assert_eq!(plan.capturable_count, 1);
    assert_eq!(plan.to_capture.len(), 1);
}

#[test]
fn equal_tag_sources_plan_without_removals() {
    use super::super::discovery::SourceSnapshotDisposition;

    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    let source_root = temp.path().join("sources");
    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    ensure_gutch_repo_layout(&repo_paths, "base").unwrap();

    let tags = manifest::SnapshotTagSet::from_compact_strings(&[
        "version=1.0.0".to_string(),
        "os=windows".to_string(),
    ])
    .unwrap();
    let candidate = |folder: &str| super::super::discovery::SourceSnapshotCandidate {
        path: source_root.join(folder).display().to_string(),
        folder_name: folder.to_string(),
        source_modified_at: String::new(),
        version_tag: String::from("1.0.0"),
        os: String::from("windows"),
        arch: String::new(),
        snapshot_tags: tags.clone(),
        disposition: SourceSnapshotDisposition::Capture,
        error: None,
    };
    let plan = plan_snapshot_sources_capture_with_context(
        &repo_root,
        Some(&source_root),
        Some(&[candidate("game-a"), candidate("game-b")]),
        None,
        &OperationControl::default(),
    )
    .unwrap();
    assert_eq!(plan.candidate_count, 2);
    assert_eq!(plan.capturable_count, 2);
    assert_eq!(plan.to_capture.len(), 2);
}

#[test]
fn pre_scan_contracts_carry_no_snapshot_id() {
    use crate::capture::contracts::SnapshotCapturePlan;

    let candidate = super::super::discovery::SourceSnapshotCandidate {
        path: "game-a".to_string(),
        folder_name: String::from("game-a"),
        source_modified_at: String::new(),
        version_tag: String::from("1.0.0"),
        os: String::from("windows"),
        arch: String::new(),
        snapshot_tags: manifest::SnapshotTagSet::from_compact_strings(&[
            "version=1.0.0".to_string()
        ])
        .unwrap(),
        disposition: super::super::discovery::SourceSnapshotDisposition::Capture,
        error: None,
    };
    let candidate_json = serde_json::to_value(&candidate).unwrap();
    assert!(
        candidate_json.get("snapshotId").is_none(),
        "candidate exposes tags as an ID: {candidate_json}"
    );

    let plan = SnapshotCapturePlan::from_candidate(candidate);
    let plan_json = serde_json::to_value(&plan).unwrap();
    assert!(
        plan_json.get("snapshotId").is_none(),
        "plan exposes tags as an ID: {plan_json}"
    );
}

#[test]
fn a_missing_sources_path_is_an_error_not_an_empty_plan() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    ensure_gutch_repo_layout(&RepoPaths::from_repo_root(&repo_root), "base").unwrap();
    let missing = temp.path().join("missing");
    assert!(
        plan_snapshot_sources_capture(&repo_root, Some(&missing), &OperationControl::default())
            .is_err()
    );
    assert!(capture_snapshot_sources(&repo_root, &missing, &OperationControl::default()).is_err());
}

#[test]
fn sources_capture_uses_the_repository_artifact_profile() {
    use crate::artifact::profiles::{
        ArtifactStorageMode, BundleSizingPreset, ChunkSizingPreset, CompressionKind,
    };
    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path().join("repo");
    let repo_paths = RepoPaths::from_repo_root(&repo_root);
    ensure_gutch_repo_layout(&repo_paths, "base").unwrap();
    let (_, mut repo_manifest) = load_gutch_repo_manifest(&repo_root).unwrap();
    repo_manifest.set_artifact_profile(
        ChunkSizingPreset::Chunk128KiB,
        CompressionKind::Lz4,
        None,
        BundleSizingPreset::Bundle8MiB,
        ArtifactStorageMode::LooseChunks,
    );
    crate::repo::paths::write_gutch_repo_manifest(&repo_paths, &repo_manifest).unwrap();
    let sources = temp.path().join("sources");
    std::fs::create_dir_all(sources.join("game")).unwrap();
    std::fs::write(sources.join("game").join("game.exe"), b"content").unwrap();

    let result =
        capture_snapshot_sources(&repo_root, &sources, &OperationControl::default()).unwrap();
    assert_eq!(result.captured_count, 1);
    let count = |root: &Path| {
        walkdir::WalkDir::new(root)
            .into_iter()
            .filter(|entry| entry.as_ref().unwrap().file_type().is_file())
            .count()
    };
    assert!(count(&repo_paths.chunks_root) > 0);
    assert_eq!(count(&repo_paths.bundles_root), 0);
}
