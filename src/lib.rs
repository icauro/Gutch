#![warn(clippy::all)]
#![warn(rust_2024_compatibility)]

extern crate self as gutch;

pub mod artifact;
pub mod build;
pub mod cli;
pub mod config;
pub mod core;
pub mod error;
pub mod helpers;
mod snapshot_ops;
pub(crate) use snapshot_ops::snapshot_manifest_from_download;
pub mod manifest;
mod progress;
pub mod reconstruct;
pub mod remote;
pub mod repo;
pub mod runtime;
pub mod snapshot;
#[cfg(test)]
mod test_util;
pub mod verify;

pub use artifact::hash::ChunkHash;
pub use artifact::{bundle, chunk, compress, hash};
pub use build::scan as build_scan;
pub use build::types as build_types;
pub use snapshot::path_guard;
pub use snapshot::source_config;

pub use build::types::{
    BuildConfig, BuildProgressSink, BuildResult, BuildTimings, BuiltBundle, BuiltLooseChunk,
    ChunkPayload, ExistingChunkLocation,
};
pub use build::{build_from_directory, build_with_config};
pub use error::{Error, Result};
pub use manifest::launch::{
    ResolvedLaunchAction, ResolvedLaunchConfiguration, validate_launch_path,
    validate_resolved_launch_for_snapshot,
};
pub use manifest::template::{
    TaggingRuleTemplate, list_templates, template_by_name, template_schema,
};
pub use manifest::{
    EditableSnapshotTagDefinition, ExternalProductDependencyRequirement, InferenceHandler,
    SnapshotTagSchemaValidationError, SnapshotTagSchemaValidationResult,
    sanitize_editable_snapshot_tag_schema, validate_editable_snapshot_tag_schema,
};
pub use progress::{
    GutchCancellationHook, GutchOperation, GutchOperationControl, GutchProgressEvent,
    GutchProgressPhase, GutchProgressSink,
};
pub use reconstruct::{
    ReconstructOptions, ReconstructTimings, VerifiedSourceTree, reconstruct_from_bundle_paths,
    reconstruct_from_repo, reconstruct_from_snapshot_manifest,
};
pub use repo::{
    GutchRepoPaths, PullConflictPolicy, REPO_INIT_NEEDS_CONFIRMATION, RemoteRepoEntry,
    RepoCheckSnapshotStatus, RepoCheckStatus, RepoCheckTimings, RepoInitAssessment,
    RepoInitOptions, RepoInitPathKind, RepoInitRisk, RepoPullPlan, RepoPullStats,
    RepoSnapshotCreateRequest, RepoSnapshotCreateResult, RepoSyncPlan, RepoSyncSnapshotChange,
    RepoSyncUpStats, RepoVerifyResult, RepoVerifyTimings, SnapshotCreateKind, SnapshotCreatePlan,
    SnapshotSourcesRefresh, SnapshotSourcesSyncOptions, SnapshotSourcesSyncPlan,
    SnapshotSourcesSyncResult, SnapshotVerifyResult, SourceSnapshotOperationCandidate,
    SourceSnapshotOperationDisposition, TrackedRepoCheckStatus, TrackedRepoCheckTimings,
    add_built_bundles_to_chunk_index, add_built_loose_chunks_to_chunk_index, assess_repo_init_path,
    bind_gutch_repo_remote, bundle_id_from_bundle_path, bundle_path, bundle_path_in_root,
    check_gutch_repo, check_tracked_gutch_repo, chunk_path, chunk_path_in_root,
    cleanup_orphan_bundles_in_repo, compute_snapshot_id_from_entries, create_repo_snapshot,
    delete_repo_snapshot_and_cleanup, delete_repo_snapshots,
    delete_repo_snapshots_with_matching_tags, describe_snapshot_create_plan,
    describe_snapshot_sources_sync_plan, discover_source_snapshot_operations,
    ensure_gutch_repo_layout, ensure_gutch_repo_tracked, initialize_tracked_gutch_repo,
    initialize_tracked_gutch_repo_with_options, link_repo, list_remote_repos, load_chunk_index,
    load_gutch_repo_manifest, load_snapshot_manifest, load_snapshot_product_metadata,
    parse_repo_init_needs_confirmation_error, plan_repo_pull, plan_snapshot_create,
    plan_snapshot_sources_sync, plan_sync_up_repo, preview_source_snapshot_ops, pull_repo,
    remove_snapshot_from_repo, repo_init_needs_confirmation_error, save_snapshot_product_metadata,
    save_snapshot_product_metadata_for_snapshots,
    serialize_snapshot_metadata_for_remote, serialize_snapshot_product_metadata_for_remote,
    snapshot_blueprint_path, snapshot_metadata_path, snapshot_product_metadata_path,
    sync_snapshot_sources, sync_up_repo, unlink_repo, update_gutch_repo_name,
    update_gutch_repo_schema, verify_gutch_repo, write_chunk_index, write_gutch_repo_manifest,
    write_snapshot_manifest_artifacts, write_snapshot_product_metadata,
};
pub use runtime::{
    ChunkStorageMode, EffectiveMemoryMode, MemoryMode, RuntimePlan, RuntimeResourceHints,
    plan_build_runtime, plan_reconstruct_runtime, plan_verify_runtime,
};
pub use snapshot::identity::{
    ResolvedSnapshotIdentity, parse_cli_tags, resolve_snapshot_identity,
    resolve_snapshot_identity_from_rules, resolve_snapshot_identity_with_inference,
};
pub use snapshot::path_guard::{
    ExistingDestinationPathKind, resolve_existing_destination_path, validate_manifest_relative_path,
};
pub use snapshot_ops::{
    GutchBundleDownload, ResolvedSnapshotOperationState, SnapshotDownloadAndReconstructRequest,
    SnapshotDownloadAndReconstructResult, SnapshotFetchRequest, SnapshotFetchResult,
    SnapshotReconstructFromRepoRequest, SnapshotReconstructFromRepoResult,
    SnapshotReconstructResult, SnapshotReconstructSource, SnapshotRepairFromRepoRequest,
    SnapshotRepairFromRepoResult, SnapshotRepairPlanFromRepoRequest,
    SnapshotRepairPlanFromRepoResult, SnapshotRepairResult, SnapshotVerifyFromRepoRequest,
    SnapshotVerifyFromRepoResult, default_snapshot_staging_dir, download_and_reconstruct_snapshot,
    fetch_snapshot, fetch_snapshot_for_required_bundle_ids, plan_repair_snapshot_from_repo,
    reconstruct_snapshot_from_repo, repair_snapshot_from_repo, resolve_snapshot_state,
    verify_snapshot_from_repo,
};
pub use verify::{
    RepairClassification, RepairIssue, RepairIssueKind, VerifyMode, VerifyRequest, VerifyTimings,
    classify_directory, estimate_build, verify_directory,
};

#[cfg(test)]
mod tests;
