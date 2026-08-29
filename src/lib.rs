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
pub use snapshot_ops::snapshot_manifest_from_download;
pub mod manifest;
mod progress;
pub mod reconstruct;
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
    SnapshotDirectRuntimeMetadata, SnapshotHostVariant, SnapshotHostVariantRuntimeMetadata,
    SnapshotLaunchAction, SnapshotProductMetadata, SnapshotRuntimeMetadata,
    SnapshotTagSchemaValidationError, SnapshotTagSchemaValidationResult,
    sanitize_editable_snapshot_tag_schema, validate_editable_snapshot_tag_schema,
};
pub use progress::{
    GutchCancellationHook, GutchOperation, GutchOperationControl, GutchProgressEvent,
    GutchProgressPhase, GutchProgressSink, emit_progress,
};
pub use reconstruct::{
    ReconstructOptions, ReconstructTimings, VerifiedSourceTree, reconstruct_from_bundle_paths,
    reconstruct_from_repo, reconstruct_from_snapshot_manifest,
};
pub use repo::remote::{
    PullConflictPolicy, RemoteArtifactSource, RemoteBundleSource, RemoteRepository,
    RemoteRepositoryProvider, RemoteRepositoryReader, RemoteRepositoryWriter, RemoteSnapshot,
    RemoteSnapshotSources, RepoPullPlan, RepoPullStats, RepoSyncPlan, RepoSyncSnapshotChange,
    RepoSyncUpStats, linked_remote_repo_id, plan_pull, plan_pull_with_provider, plan_sync_up,
    plan_sync_up_with_provider, pull_repo, sync_up_repo,
};
pub use repo::tag_requirements::{TagRequirement, TagRequirementStatus, compare_tag_requirements};
pub use repo::{
    GutchRepoPaths, REPO_INIT_NEEDS_CONFIRMATION, RepoCheckSnapshotStatus, RepoCheckStatus,
    RepoCheckTimings, RepoInitAssessment, RepoInitOptions, RepoInitPathKind, RepoInitRisk,
    RepoSnapshotCaptureRequest, RepoSnapshotCaptureResult, RepoVerifyResult, RepoVerifyTimings,
    SnapshotCaptureKind, SnapshotCapturePlan, SnapshotSourcesCaptureOptions,
    SnapshotSourcesCapturePlan, SnapshotSourcesCaptureResult, SnapshotSourcesRefresh,
    SnapshotVerifyResult, SourceSnapshotOperationCandidate, SourceSnapshotOperationDisposition,
    add_built_bundles_to_chunk_index, add_built_loose_chunks_to_chunk_index, assess_repo_init_path,
    bind_gutch_repo_remote, bundle_id_from_bundle_path, bundle_path, bundle_path_in_root,
    capture_repo_snapshot, capture_snapshot_sources, check_gutch_repo, chunk_path,
    chunk_path_in_root, cleanup_orphan_bundles_in_repo, compute_snapshot_id_from_entries,
    delete_repo_snapshot_and_cleanup, delete_repo_snapshots,
    delete_repo_snapshots_with_matching_tags, describe_snapshot_capture_plan,
    describe_snapshot_sources_capture_plan, discover_source_snapshot_operations,
    ensure_gutch_repo_layout, initialize_gutch_repo, initialize_gutch_repo_with_options,
    load_chunk_index, load_gutch_repo_manifest, load_snapshot_manifest,
    load_snapshot_product_metadata, parse_repo_init_needs_confirmation_error,
    plan_snapshot_capture, plan_snapshot_sources_capture, preview_source_snapshot_ops,
    remove_snapshot_from_repo, repo_init_needs_confirmation_error, save_snapshot_product_metadata,
    save_snapshot_product_metadata_for_snapshots, serialize_snapshot_metadata_for_remote,
    serialize_snapshot_product_metadata_for_remote, snapshot_blueprint_path,
    snapshot_metadata_path, snapshot_product_metadata_path, update_gutch_repo_name,
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
    BundleDownload, BundleUrlProvider, ResolvedSnapshotOperationState,
    SnapshotDownloadAndReconstructRequest, SnapshotDownloadAndReconstructResult,
    SnapshotFetchRequest, SnapshotFetchResult, SnapshotReconstructFromRepoRequest,
    SnapshotReconstructFromRepoResult, SnapshotReconstructResult, SnapshotReconstructSource,
    SnapshotRepairFromRepoRequest, SnapshotRepairFromRepoResult, SnapshotRepairPlanFromRepoRequest,
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
