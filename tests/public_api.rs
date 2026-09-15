//! Host-facing API contracts; broader CLI workflows live in Python.
use gutch::api::{
    Error, GutchCancellationHook, GutchOperationControl, InstallationRequest, RuntimeResourceHints,
    reconstruct_snapshot,
};
use std::sync::Arc;

#[test]
fn comparison_mismatch_returns_to_the_embedding_host() {
    let root = tempfile::tempdir().unwrap();
    let expected = root.path().join("expected.txt");
    let actual = root.path().join("actual.txt");
    std::fs::write(&expected, b"expected").unwrap();
    std::fs::write(&actual, b"different").unwrap();
    assert!(
        !gutch::api::compare_paths(&expected, &actual)
            .unwrap()
            .compare_passed
    );
    let result = gutch::api::run_from_args([
        "gutch",
        "path",
        "compare",
        "--expected",
        expected.to_str().unwrap(),
        "--actual",
        actual.to_str().unwrap(),
    ]);
    assert!(matches!(result, Err(Error::Verify(_))), "{result:?}");
}

#[test]
fn api_and_cli_capture_preserve_equivalent_content_tags_and_profiles() {
    use gutch::api::*;
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("content"), b"content").unwrap();
    initialize_repo(&repo, RepoInitOptions::default()).unwrap();
    let captured = capture_snapshot(SnapshotCaptureRequest {
        repo_root: repo.clone(),
        source_dir: source.clone(),
        explicit_tags: SnapshotTagSet::from_compact_strings(&["version=1.0.0".to_string()])
            .unwrap(),
        chunk_profile: Default::default(),
        bundle_profile: Default::default(),
        compression_algorithm: CompressionKind::Lz4,
        compression_level: None,
        storage_mode: ArtifactStorageMode::LooseChunks,
        runtime_hints: RuntimeResourceHints { jobs: Some(1), ..Default::default() },
        control: Default::default(),
    })
    .unwrap();
    let (_, manifest) = load_gutch_repo_manifest(&repo).unwrap();
    assert_eq!(manifest.artifact_profile.compression, CompressionKind::Lz4);
    assert_eq!(
        manifest.artifact_profile.storage,
        ArtifactStorageMode::LooseChunks
    );
    let cli_repo = root.path().join("cli-repo");
    initialize_repo(&cli_repo, RepoInitOptions::default()).unwrap();
    run_from_args([
        "gutch",
        "snapshot",
        "capture",
        "--repo",
        cli_repo.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--yes",
        "--jobs",
        "1",
        "--tag",
        "version=1.0.0",
        "--compression-algorithm",
        "lz4",
        "--storage-mode",
        "loose-chunks",
    ])
    .unwrap();
    let (_, cli_manifest) = load_gutch_repo_manifest(&cli_repo).unwrap();
    assert_eq!(
        serde_json::to_value(&manifest.artifact_profile).unwrap(),
        serde_json::to_value(&cli_manifest.artifact_profile).unwrap()
    );
    let metadata_path = |repository: &std::path::Path| {
        repository
            .join("snapshots")
            .join(&captured.snapshot_id)
            .join("snapshot_metadata.json")
    };
    let api_metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(metadata_path(&repo)).unwrap()).unwrap();
    let cli_metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(metadata_path(&cli_repo)).unwrap()).unwrap();
    assert_eq!(
        api_metadata["tags"],
        serde_json::json!({"key_value": {"version": "1.0.0"}})
    );
    assert_eq!(api_metadata["tags"], cli_metadata["tags"]);
    std::fs::remove_dir_all(&source).unwrap();
    for (repository, label) in [(repo, "api-output"), (cli_repo, "cli-output")] {
        let destination = root.path().join(label);
        reconstruct_snapshot(InstallationRequest {
            repo_path: repository,
            snapshot_id: captured.snapshot_id.clone(),
            destination_dir: destination.clone(),
            staging_dir: None,
            source_snapshot_id: None,
            source_dir: None,
            runtime_hints: RuntimeResourceHints { jobs: Some(1), ..Default::default() },
            control: Default::default(),
        })
        .unwrap();
        assert_eq!(
            std::fs::read(destination.join("content")).unwrap(),
            b"content"
        );
        assert_eq!(std::fs::read_dir(destination).unwrap().count(), 1);
    }
}

#[derive(Debug)]
struct Cancelled;

impl GutchCancellationHook for Cancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

#[test]
fn cancelled_reconstruction_preserves_destination_and_does_not_create_staging() {
    let temporary = tempfile::tempdir().unwrap();
    let destination = temporary.path().join("installed");
    let staging = temporary.path().join("staging");
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(destination.join("keep.txt"), b"prior installation").unwrap();
    let result = reconstruct_snapshot(InstallationRequest {
        repo_path: temporary.path().join("unused-repository"),
        snapshot_id: "a".repeat(64),
        destination_dir: destination.clone(),
        staging_dir: Some(staging.clone()),
        source_snapshot_id: None,
        source_dir: None,
        runtime_hints: RuntimeResourceHints::default(),
        control: GutchOperationControl::default().with_cancellation(Arc::new(Cancelled)),
    });
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert_eq!(
        std::fs::read(destination.join("keep.txt")).unwrap(),
        b"prior installation"
    );
    assert_eq!(std::fs::read_dir(destination).unwrap().count(), 1);
    assert!(!staging.exists());
}
