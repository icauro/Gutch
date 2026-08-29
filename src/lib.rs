#![warn(clippy::all)]
#![warn(rust_2024_compatibility)]

extern crate self as gutch;

mod artifact;
mod build;
pub mod cli;
mod config;
mod error;
mod helpers;
mod manifest;
mod progress;
mod reconstruct;
mod repo;
mod runtime;
mod snapshot;
mod snapshot_ops;
#[cfg(test)]
mod test_util;
mod transport;
mod verify;

pub(crate) use artifact::{bundle, chunk, compress, hash};
pub(crate) use build::scan as build_scan;
pub(crate) use build::types as build_types;
pub(crate) use snapshot::path_guard;
pub(crate) use snapshot::source_config;

pub use artifact::hash::ChunkHash;
pub use artifact::hash::{hash_chunk, hash_hex};
pub use config::{GutchConfig, TrackedRepo, load_config, save_config};
pub use helpers::{encode_query_component, read_response_bounded};
pub use snapshot_ops::snapshot_manifest_from_download;

pub use build::pipeline::{build_from_directory, build_with_config};
pub use build::types::{
    BuildConfig, BuildResult, BuildTimings, BuiltBundle, BuiltLooseChunk, ChunkPayload,
    ExistingChunkLocation,
};
pub use error::{Error, Result};
pub use manifest::launch::{
    ResolvedLaunchAction, ResolvedLaunchConfiguration, validate_launch_path,
    validate_resolved_launch_for_snapshot,
};
pub use manifest::template::{
    TaggingRuleTemplate, list_templates, template_by_name, template_schema,
};
pub use manifest::{
    ArtifactSizingPreset, ArtifactStorageMode, BundleSizingPreset, CompressionPreset,
    EditableSnapshotTagDefinition, ExternalProductDependencyRequirement, InferenceHandler,
    SnapshotDirectRuntimeMetadata, SnapshotHostVariant, SnapshotHostVariantRuntimeMetadata,
    SnapshotLaunchAction, SnapshotProductMetadata, SnapshotRuntimeMetadata, SnapshotTagDefinition,
    SnapshotTagKind, SnapshotTagSchema, SnapshotTagSchemaValidationError,
    SnapshotTagSchemaValidationResult, SnapshotTagSet, StructuredVersion,
    sanitize_editable_snapshot_tag_schema, validate_editable_snapshot_tag_schema,
};
pub use progress::{
    GutchCancellationHook, GutchOperation, GutchOperationControl, GutchProgressEvent,
    GutchProgressPhase, GutchProgressSink, emit_progress,
};
pub use reconstruct::{
    ReconstructOptions, ReconstructRequest, ReconstructTimings, VerifiedSourceTree,
    reconstruct_from_bundle_paths, reconstruct_from_repo, reconstruct_from_snapshot_manifest,
};
pub use repo::capture_plan::{
    RepoSnapshotCaptureRequest, RepoSnapshotCaptureResult, SnapshotCaptureKind,
    SnapshotCapturePlan, capture_repo_snapshot, delete_repo_snapshot_and_cleanup,
    delete_repo_snapshots, delete_repo_snapshots_with_matching_tags,
    describe_snapshot_capture_plan, plan_snapshot_capture,
};
pub use repo::check::{
    RepoCheckSnapshotStatus, RepoCheckStatus, RepoCheckTimings, SourceSnapshotOperationCandidate,
    SourceSnapshotOperationDisposition, check_gutch_repo, discover_source_snapshot_operations,
    preview_source_snapshot_ops,
};
pub use repo::chunk_index::{
    add_built_bundles_to_chunk_index, add_built_loose_chunks_to_chunk_index, load_chunk_index,
    write_chunk_index,
};
pub use repo::cleanup::{cleanup_orphan_bundles_in_repo, remove_snapshot_from_repo};
pub use repo::init_policy::{
    REPO_INIT_NEEDS_CONFIRMATION, RepoInitAssessment, RepoInitOptions, RepoInitPathKind,
    RepoInitRisk, assess_repo_init_path, initialize_gutch_repo_with_options,
    parse_repo_init_needs_confirmation_error, repo_init_needs_confirmation_error,
};
pub use repo::paths::{
    GutchRepoPaths, bundle_id_from_bundle_path, bundle_path, bundle_path_in_root, chunk_path,
    chunk_path_in_root, ensure_gutch_repo_layout, load_gutch_repo_manifest,
    write_gutch_repo_manifest,
};
pub use repo::remote::{
    PullConflictPolicy, RemoteArtifactSource, RemoteBundleSource, RemoteRepository,
    RemoteRepositoryCommandProvider, RemoteRepositoryProvider, RemoteRepositoryReader,
    RemoteRepositorySummary, RemoteRepositoryWriter, RemoteSnapshot, RemoteSnapshotSources,
    RemoteTagRequirementsResult, RepoPullPlan, RepoPullStats, RepoSyncPlan, RepoSyncSnapshotChange,
    RepoSyncUpStats, bind_repo_remote, inspect_repo_tag_requirements, link_repo,
    linked_remote_repo_id, list_remote_repositories, plan_pull, plan_pull_with_provider,
    plan_sync_up, plan_sync_up_with_provider, pull_repo, sync_up_repo, unlink_repo,
};
pub use repo::snapshot_manifests::{
    load_snapshot_manifest, load_snapshot_product_metadata, save_snapshot_product_metadata,
    save_snapshot_product_metadata_for_snapshots, serialize_snapshot_metadata_for_remote,
    serialize_snapshot_product_metadata_for_remote, snapshot_blueprint_path,
    snapshot_metadata_path, snapshot_product_metadata_path, write_snapshot_manifest_artifacts,
    write_snapshot_product_metadata,
};
pub use repo::snapshot_sources::{
    SnapshotSourcesCaptureOptions, SnapshotSourcesCapturePlan, SnapshotSourcesCaptureResult,
    SnapshotSourcesRefresh, capture_snapshot_sources, describe_snapshot_sources_capture_plan,
    plan_snapshot_sources_capture, plan_snapshot_sources_capture_with_context,
};
pub use repo::source_identity::compute_snapshot_id_from_entries;
pub use repo::tag_requirements::{TagRequirement, TagRequirementStatus, compare_tag_requirements};
pub use repo::updates::{initialize_gutch_repo, update_gutch_repo_name, update_gutch_repo_schema};
pub use repo::verify::{
    RepoVerifyResult, RepoVerifyTimings, SnapshotVerifyResult, verify_gutch_repo,
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
