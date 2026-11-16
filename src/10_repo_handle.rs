//! A repository handle with one-line calls for the common operations.
//!
//! Each method fills in a request with its defaults and calls the matching
//! function in `gutch::api`; for any other option, build that request
//! directly.

use std::path::{Path, PathBuf};

use crate::capture::contracts::{SnapshotCaptureRequest, SnapshotCaptureResult};
use crate::installation::{
    InstallationReconstructRequest, InstallationReconstructResult, InstallationRepairRequest,
    InstallationRepairResult, InstallationVerifyRequest, InstallationVerifyResult,
};
use crate::manifest::SnapshotTagSet;
use crate::repo::file_read::{SnapshotFileReadRequest, read_snapshot_file};
use crate::repo::init_policy::{RepoInitRequest, init_repo};
use crate::repo::paths::RepoPaths;
use crate::repo::snapshot_manifests::{SnapshotDiffRequest, diff_snapshots};
use crate::snapshot::diff::SnapshotDiffResult;
use crate::sync::remote::{
    RemoteRepoProvider, RemoteRepoReader, RepoPullRequest, RepoPullResult, RepoPushRequest,
    RepoPushResult, SnapshotFileFetchRequest, fetch_snapshot_file, pull_repo, push_repo,
};
use crate::{Error, Result};

/// A Gutch repository on disk.
///
/// ```no_run
/// # fn main() -> gutch::api::Result<()> {
/// use gutch::api::Repo;
///
/// let repo = Repo::init("work/repo")?;
/// let snapshot_id = repo.capture("build", &[("version", "1.0")])?.snapshot_id;
/// repo.reconstruct(&snapshot_id, "work/app")?;
/// println!("{}", repo.verify(&snapshot_id, "work/app")?.issues);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// Creates a repository in a folder that is missing or empty.
    pub fn init(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        init_repo(&RepoInitRequest {
            repo_root: root.clone(),
            ..Default::default()
        })?;
        Ok(Self { root })
    }

    /// Opens an existing repository.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !RepoPaths::from_repo_root(&root)
            .repo_metadata_path
            .is_file()
        {
            return Err(Error::repo(format!(
                "no Gutch repository at {}",
                root.display()
            )));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Captures `source`, a folder or a single file, as a snapshot with the
    /// given `key=value` tags.
    pub fn capture(
        &self,
        source: impl AsRef<Path>,
        tags: &[(&str, &str)],
    ) -> Result<SnapshotCaptureResult> {
        crate::capture::sources::capture_snapshot(&SnapshotCaptureRequest {
            repo_root: self.root.clone(),
            source: source.as_ref().to_path_buf(),
            tags: SnapshotTagSet::from_pairs(tags.iter().copied())?,
            ..Default::default()
        })
    }

    /// Installs a snapshot into `destination_dir`, replacing what is there.
    pub fn reconstruct(
        &self,
        snapshot_id: &str,
        destination_dir: impl AsRef<Path>,
    ) -> Result<InstallationReconstructResult> {
        crate::installation::reconstruct_installation(&InstallationReconstructRequest {
            repo_root: self.root.clone(),
            snapshot_id: snapshot_id.to_owned(),
            destination_dir: destination_dir.as_ref().to_path_buf(),
            ..Default::default()
        })
    }

    /// Checks an installation against a snapshot without changing it.
    pub fn verify(
        &self,
        snapshot_id: &str,
        destination_dir: impl AsRef<Path>,
    ) -> Result<InstallationVerifyResult> {
        crate::installation::verify_installation(&InstallationVerifyRequest {
            repo_root: self.root.clone(),
            snapshot_id: snapshot_id.to_owned(),
            destination_dir: destination_dir.as_ref().to_path_buf(),
            ..Default::default()
        })
    }

    /// Restores missing and corrupt files; extra files are kept.
    pub fn repair(
        &self,
        snapshot_id: &str,
        destination_dir: impl AsRef<Path>,
    ) -> Result<InstallationRepairResult> {
        crate::installation::repair_installation(&InstallationRepairRequest {
            repo_root: self.root.clone(),
            snapshot_id: snapshot_id.to_owned(),
            destination_dir: destination_dir.as_ref().to_path_buf(),
            ..Default::default()
        })
    }

    /// Lists what changed from one snapshot to another.
    pub fn diff(&self, from_snapshot_id: &str, to_snapshot_id: &str) -> Result<SnapshotDiffResult> {
        diff_snapshots(&SnapshotDiffRequest {
            repo_root: self.root.clone(),
            from_snapshot_id: from_snapshot_id.to_owned(),
            to_snapshot_id: to_snapshot_id.to_owned(),
            ..Default::default()
        })
    }

    /// Returns one file of a snapshot without installing it; `path` uses `/`
    /// separators. See [`read_snapshot_file`].
    pub fn read_file(&self, snapshot_id: &str, path: &str) -> Result<Vec<u8>> {
        read_snapshot_file(&SnapshotFileReadRequest {
            repo_root: self.root.clone(),
            snapshot_id: snapshot_id.to_owned(),
            path: path.to_owned(),
            ..Default::default()
        })
    }

    /// Reads one file of a snapshot on the bound remote, downloading the
    /// snapshot's metadata and only the bundles that hold the file.
    pub fn fetch_file(
        &self,
        remote: &dyn RemoteRepoReader,
        snapshot_id: &str,
        path: &str,
    ) -> Result<Vec<u8>> {
        fetch_snapshot_file(
            &SnapshotFileFetchRequest {
                repo_root: self.root.clone(),
                snapshot_id: snapshot_id.to_owned(),
                path: path.to_owned(),
                ..Default::default()
            },
            remote,
        )
    }

    /// Pushes to the remote repository this one is bound to.
    pub fn push(&self, remote: &dyn RemoteRepoProvider) -> Result<RepoPushResult> {
        push_repo(
            &RepoPushRequest {
                repo_root: self.root.clone(),
                ..Default::default()
            },
            remote,
        )
    }

    /// Pulls from the remote repository this one is bound to.
    pub fn pull(&self, remote: &dyn RemoteRepoReader) -> Result<RepoPullResult> {
        pull_repo(
            &RepoPullRequest {
                repo_root: self.root.clone(),
                ..Default::default()
            },
            remote,
        )
    }
}

#[cfg(test)]
#[path = "10_repo_handle_tests.rs"]
mod tests;
