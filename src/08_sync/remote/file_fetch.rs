//! Reads one file of a remote snapshot, downloading only the bundles that hold it.

use super::provider::RemoteRepoReader;
use super::pull::{PullBundles, RepoPullRequest, pull_repo};
use crate::Result;
use crate::progress::OperationControl;
use crate::repo::file_read::{SnapshotFileReadRequest, read_snapshot_file};
use std::path::PathBuf;

/// One file of a remote snapshot to read through a local repository.
#[derive(Debug, Clone, Default)]
pub struct SnapshotFileFetchRequest {
    /// Local repository that keeps the downloaded metadata and bundles.
    pub repo_root: PathBuf,
    /// Remote repository to fetch from; `None` uses the bound remote.
    pub remote_repo_id: Option<String>,
    pub snapshot_id: String,
    /// The file's path inside the snapshot, with `/` separators.
    pub path: String,
    pub control: OperationControl,
}

/// Pulls the snapshot's metadata and only the bundles holding the file, then
/// returns the file's checked content. Other snapshots are not downloaded,
/// and bundles already in the local repository are not downloaded again.
pub fn fetch_snapshot_file(
    request: &SnapshotFileFetchRequest,
    provider: &dyn RemoteRepoReader,
) -> Result<Vec<u8>> {
    pull_repo(
        &RepoPullRequest {
            repo_root: request.repo_root.clone(),
            remote_repo_id: request.remote_repo_id.clone(),
            snapshot_ids: vec![request.snapshot_id.clone()],
            bundles: PullBundles::Files(vec![request.path.clone()]),
            control: request.control.clone(),
            ..Default::default()
        },
        provider,
    )?;
    read_snapshot_file(&SnapshotFileReadRequest {
        repo_root: request.repo_root.clone(),
        snapshot_id: request.snapshot_id.clone(),
        path: request.path.clone(),
        control: request.control.clone(),
    })
}
