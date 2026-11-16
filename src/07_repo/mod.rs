//! Repository layouts, guarded persistence, artifact indexes, and recovery.

pub(crate) mod artifacts;
pub(crate) mod cleanup;
pub(crate) mod coordination;
pub(crate) mod index;
pub(crate) mod init_policy;
pub(crate) mod inventory;
pub(crate) mod paths;
pub(crate) mod read;
pub(crate) mod recovery;
pub(crate) mod snapshot_manifests;
pub(crate) mod snapshot_tag_summary;
pub(crate) mod snapshots;
pub(crate) mod status;
pub(crate) mod tag_references;
pub(crate) mod updates;
pub(crate) mod verify;

pub(crate) use crate::Result;
pub(crate) use snapshot_tag_summary::summarize_snapshot_tags;
pub(crate) use status::RepoStatus;
pub(crate) use status::repo_status;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
