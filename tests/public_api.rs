//! Host-facing API contracts; broader CLI workflows live in Python.
use gutch::api::{
    Error, GutchCancellationHook, GutchOperationControl, RuntimeResourceHints,
    SnapshotReconstructFromRepoRequest, reconstruct_snapshot_from_repo,
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
fn direct_capture_persists_packaging_profile_without_the_cli() {
    use gutch::api::*;
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("content"), b"content").unwrap();
    initialize_gutch_repo_with_options(&repo, RepoInitOptions::default()).unwrap();
    capture_repo_snapshot(RepoSnapshotCaptureRequest {
        repo_root: repo.clone(),
        source_dir: source,
        explicit_tags: Default::default(),
        chunk_profile: Default::default(),
        bundle_profile: Default::default(),
        compression_algorithm: CompressionKind::Lz4,
        compression_level: None,
        storage_mode: ArtifactStorageMode::LooseChunks,
        runtime_hints: RuntimeResourceHints { jobs: Some(1) },
        control: Default::default(),
    })
    .unwrap();
    let (_, manifest) = load_gutch_repo_manifest(&repo).unwrap();
    assert_eq!(manifest.artifact_profile.compression, CompressionKind::Lz4);
    assert_eq!(
        manifest.artifact_profile.storage,
        ArtifactStorageMode::LooseChunks
    );
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
    let result = reconstruct_snapshot_from_repo(SnapshotReconstructFromRepoRequest {
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
