mod check;
mod chunk_index;
mod cleanup;
mod create_plan;
mod init_policy;
mod inventory;
mod paths;
mod pull;
mod remote_ops;
mod snapshot_descriptor;
mod snapshot_manifests;
mod snapshot_sources;
mod source_identity;
mod updates;
mod verify;

pub use crate::Result;
pub use check::{
    RepoCheckSnapshotStatus, RepoCheckStatus, RepoCheckTimings, SourceSnapshotOperationCandidate,
    SourceSnapshotOperationDisposition, TrackedRepoCheckStatus, TrackedRepoCheckTimings,
    check_gutch_repo, check_tracked_gutch_repo, discover_source_snapshot_operations,
    ensure_gutch_repo_tracked, preview_source_snapshot_ops,
};
pub use chunk_index::*;
pub use cleanup::*;
pub use create_plan::{
    RepoSnapshotCreateRequest, RepoSnapshotCreateResult, SnapshotCreateKind, SnapshotCreatePlan,
    create_repo_snapshot, delete_repo_snapshot_and_cleanup, delete_repo_snapshots,
    delete_repo_snapshots_with_matching_tags, describe_snapshot_create_plan, plan_snapshot_create,
};
pub use init_policy::{
    REPO_INIT_NEEDS_CONFIRMATION, RepoInitAssessment, RepoInitOptions, RepoInitPathKind,
    RepoInitRisk, assess_repo_init_path, initialize_tracked_gutch_repo_with_options,
    parse_repo_init_needs_confirmation_error, repo_init_needs_confirmation_error,
};
pub use paths::*;
pub use pull::*;
pub use remote_ops::{
    RepoSyncPlan, RepoSyncSnapshotChange, RepoSyncUpStats, link_repo, plan_sync_up_repo,
    sync_up_repo, unlink_repo,
};
pub use snapshot_manifests::*;
pub use snapshot_sources::{
    SnapshotSourcesRefresh, SnapshotSourcesSyncOptions, SnapshotSourcesSyncPlan,
    SnapshotSourcesSyncResult, describe_snapshot_sources_sync_plan, plan_snapshot_sources_sync,
    sync_snapshot_sources,
};
pub use source_identity::*;
pub use updates::*;
pub use verify::{RepoVerifyResult, RepoVerifyTimings, SnapshotVerifyResult, verify_gutch_repo};

#[cfg(test)]
#[path = "repo_tests.rs"]
mod tests;
