pub(crate) mod capture_plan;
pub(crate) mod capture_sources;
pub(crate) mod check;
pub(crate) mod chunk_index;
pub(crate) mod cleanup;
pub(crate) mod init_policy;
mod inventory;
pub(crate) mod paths;
pub(crate) mod remote;
mod snapshot_descriptor;
pub(crate) mod snapshot_manifests;
pub(crate) mod snapshot_sources;
pub(crate) mod source_identity;
pub(crate) mod tag_references;
pub(crate) mod updates;
pub(crate) mod verify;

pub(crate) use crate::Result;
pub(crate) use check::{
    RepoCheckStatus, SourceSnapshotOperationCandidate, check_gutch_repo,
    discover_source_snapshot_operations,
};
pub(crate) use chunk_index::*;
pub(crate) use cleanup::*;
pub(crate) use paths::*;
pub(crate) use snapshot_descriptor::summarize_snapshot_tags;
pub(crate) use snapshot_manifests::*;
pub(crate) use source_identity::*;

#[cfg(test)]
#[path = "repo_tests.rs"]
mod tests;
