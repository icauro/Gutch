//! Host-facing cancellation remains a Rust contract; CLI workflows live in Python.
use gutch::api::{
    Error, GutchCancellationHook, GutchOperationControl, RuntimeResourceHints,
    SnapshotReconstructFromRepoRequest, reconstruct_snapshot_from_repo,
};
use std::sync::Arc;

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
