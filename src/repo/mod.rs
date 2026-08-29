mod capture_plan;
mod check;
mod chunk_index;
mod cleanup;
mod init_policy;
mod inventory;
mod paths;
pub mod remote;
mod snapshot_descriptor;
mod snapshot_manifests;
mod snapshot_sources;
mod source_identity;
pub mod tag_requirements;
mod updates;
mod verify;

pub use crate::Result;
pub use capture_plan::{
    RepoSnapshotCaptureRequest, RepoSnapshotCaptureResult, SnapshotCaptureKind,
    SnapshotCapturePlan, capture_repo_snapshot, delete_repo_snapshot_and_cleanup,
    delete_repo_snapshots, delete_repo_snapshots_with_matching_tags,
    describe_snapshot_capture_plan, plan_snapshot_capture,
};
pub use check::{
    RepoCheckSnapshotStatus, RepoCheckStatus, RepoCheckTimings, SourceSnapshotOperationCandidate,
    SourceSnapshotOperationDisposition, check_gutch_repo, discover_source_snapshot_operations,
    preview_source_snapshot_ops,
};
pub use chunk_index::*;
pub use cleanup::*;
pub use init_policy::{
    REPO_INIT_NEEDS_CONFIRMATION, RepoInitAssessment, RepoInitOptions, RepoInitPathKind,
    RepoInitRisk, assess_repo_init_path, initialize_gutch_repo_with_options,
    parse_repo_init_needs_confirmation_error, repo_init_needs_confirmation_error,
};
pub use paths::*;
pub use remote::*;
pub use snapshot_descriptor::summarize_snapshot_tags;
pub use snapshot_manifests::*;
pub use snapshot_sources::{
    SnapshotSourcesCaptureOptions, SnapshotSourcesCapturePlan, SnapshotSourcesCaptureResult,
    SnapshotSourcesRefresh, capture_snapshot_sources, describe_snapshot_sources_capture_plan,
    plan_snapshot_sources_capture, plan_snapshot_sources_capture_with_context,
};
pub use source_identity::*;
pub use tag_requirements::*;
pub use updates::*;
pub use verify::{RepoVerifyResult, RepoVerifyTimings, SnapshotVerifyResult, verify_gutch_repo};

#[cfg(test)]
#[path = "repo_tests.rs"]
mod tests;
