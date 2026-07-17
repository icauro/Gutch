mod check;
mod chunk_index;
mod cleanup;
mod init_policy;
mod insert_plan;
mod inventory;
mod paths;
mod remote_ops;
mod source_identity;
mod target_descriptor;
mod target_manifests;
mod target_sources;
mod updates;
mod verify;

pub use crate::Result;
pub use check::{
    RepoCheckStatus, RepoCheckTargetStatus, RepoCheckTimings, SourceTargetInstallCandidate,
    TrackedRepoCheckStatus, TrackedRepoCheckTimings, check_gutch_repo, check_tracked_gutch_repo,
    discover_source_target_installs, ensure_gutch_repo_tracked, preview_source_target_install,
};
pub use chunk_index::*;
pub use cleanup::*;
pub use init_policy::{
    REPO_INIT_NEEDS_CONFIRMATION, RepoInitAssessment, RepoInitOptions, RepoInitPathKind,
    RepoInitRisk, assess_repo_init_path, initialize_tracked_gutch_repo_with_options,
    parse_repo_init_needs_confirmation_error, repo_init_needs_confirmation_error,
};
pub use insert_plan::{
    RepoTargetInsertRequest, RepoTargetInsertResult, TargetInsertKind, TargetInsertPlan,
    delete_repo_target_and_cleanup, delete_repo_targets, delete_repo_targets_with_matching_tags,
    describe_target_insert_plan, insert_repo_target, plan_target_insert,
};
pub use paths::*;
pub use remote_ops::{
    RepoSyncPlan, RepoSyncTargetChange, RepoSyncUpStats, link_repo, plan_sync_up_repo,
    sync_up_repo, unlink_repo,
};
pub use source_identity::*;
pub use target_manifests::*;
pub use target_sources::{
    TargetSourcesRefresh, TargetSourcesSyncOptions, TargetSourcesSyncPlan, TargetSourcesSyncResult,
    describe_target_sources_sync_plan, plan_target_sources_sync, sync_target_sources,
};
pub use updates::*;
pub use verify::{RepoVerifyResult, RepoVerifyTimings, TargetVerifyResult, verify_gutch_repo};

#[cfg(test)]
#[path = "repo_tests.rs"]
mod tests;
