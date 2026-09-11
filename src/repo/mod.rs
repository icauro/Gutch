pub(crate) mod check;
pub(crate) mod cleanup;
pub(crate) mod coordination;
pub(crate) mod index;
pub(crate) mod init_policy;
mod inventory;
pub(crate) mod paths;
pub(crate) mod recovery;
pub(crate) mod remote;
pub(crate) mod snapshot_descriptor;
pub(crate) mod snapshot_manifests;
pub(crate) mod snapshots;
pub(crate) mod tag_references;
pub(crate) mod updates;
pub(crate) mod verify;

pub(crate) use crate::Result;
pub(crate) use check::RepoCheckStatus;
pub(crate) use check::SourceSnapshotOperationCandidate;
pub(crate) use check::check_gutch_repo;
pub(crate) use snapshot_descriptor::summarize_snapshot_tags;

#[cfg(test)]
#[path = "repo_tests.rs"]
mod tests;

pub(crate) mod artifacts;
