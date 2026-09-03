#![warn(clippy::all)]
#![warn(rust_2024_compatibility)]

extern crate self as gutch;

mod artifact;
mod build;
pub mod cli;
mod config;
mod error;
mod file_integrity;
mod helpers;
mod manifest;
mod profile;
mod progress;
mod repo;
mod runtime;
mod snapshot;
mod snapshot_ops;
#[cfg(test)]
mod test_util;
mod transport;
mod verify;

pub(crate) use artifact::store::{ArtifactRef, required_artifacts, required_artifacts_from_index};
pub(crate) use artifact::{bundle, chunk, compress, hash};
pub(crate) use build::scan as build_scan;
pub(crate) use build::types as build_types;
pub(crate) use snapshot::path_guard;
pub(crate) use snapshot::source_config;

pub use artifact::hash::ChunkHash;
pub use artifact::hash::{hash_chunk, hash_hex};
pub use config::{GutchConfig, TrackedRepo, load_config, save_config};
pub(crate) use snapshot_ops::snapshot_manifest_from_download;

pub(crate) use build::pipeline::build_with_config_timed_on_pool;
pub use build::pipeline::{build_from_directory, build_with_config};
pub(crate) use build::types::BuildTimings;
pub(crate) use build::types::ExistingChunkLocation;
pub use build::types::{BuildConfig, BuildResult, BuiltBundle, BuiltLooseChunk};
pub use error::{Error, Result};
pub use manifest::gutch_repo::{ArtifactProfile, GutchRepoManifest};
pub use manifest::launch::{ResolvedLaunchAction, ResolvedLaunchConfiguration};
pub use manifest::template::{TaggingRuleTemplate, list_templates, template_schema};
pub use manifest::{
    ArtifactSizingPreset, ArtifactStorageMode, BundleSizingPreset, ChunkRef, CompressionPreset,
    DirectoryManifest, EditableSnapshotTagDefinition, ExternalProductDependencyRequirement,
    FileManifest, InferenceHandler, PUBLIC_METADATA_FORMAT_VERSION, SnapshotDirectRuntimeMetadata,
    SnapshotEntry, SnapshotHostVariant, SnapshotHostVariantRuntimeMetadata, SnapshotLaunchAction,
    SnapshotManifest, SnapshotProductMetadata, SnapshotRuntimeMetadata, SnapshotTagDefinition,
    SnapshotTagKind, SnapshotTagSchema, SnapshotTagSchemaValidationError,
    SnapshotTagSchemaValidationResult, SnapshotTagSet, SnapshotTagValueDefinition,
    StructuredVersion, SymlinkManifest, sanitize_editable_snapshot_tag_schema,
    validate_editable_snapshot_tag_schema,
};
pub(crate) use profile::file_trace_args;
pub use profile::{GutchProfileSpan, GutchProfileSpanSummary, GutchProfileSummary, GutchProfiler};
pub(crate) use progress::emit_progress;
pub use progress::{
    GutchCancellationHook, GutchOperation, GutchOperationControl, GutchProgressEvent,
    GutchProgressPhase, GutchProgressSink,
};
pub use repo::capture_plan::{
    RepoSnapshotCaptureRequest, RepoSnapshotCaptureResult, SnapshotCaptureKind,
    SnapshotCapturePlan, capture_repo_snapshot, capture_repo_snapshot_with_plan,
    delete_repo_snapshot_and_cleanup, describe_snapshot_capture_plan, plan_snapshot_capture,
};
pub(crate) use repo::capture_plan::{
    capture_repo_snapshot_with_existing_store_state, delete_repo_snapshots,
    delete_repo_snapshots_with_matching_tags,
};
pub use repo::check::{
    RepoCheckSnapshotStatus, RepoCheckStatus, SourceSnapshotOperationCandidate,
    SourceSnapshotOperationDisposition, check_gutch_repo, discover_source_snapshot_operations,
};
pub(crate) use repo::chunk_index::{
    add_built_bundles_to_chunk_index, add_built_loose_chunks_to_chunk_index, load_chunk_index,
    write_chunk_index,
};
pub(crate) use repo::cleanup::{cleanup_orphan_bundles_in_repo, remove_snapshot_from_repo};
pub use repo::init_policy::{
    RepoInitAssessment, RepoInitOptions, RepoInitPathKind, RepoInitRisk, assess_repo_init_path,
    initialize_gutch_repo_with_options,
};
pub use repo::paths::{GutchRepoPaths, load_gutch_repo_manifest};
pub(crate) use repo::paths::{
    bundle_id_from_bundle_path, bundle_path, bundle_path_in_root, chunk_path, chunk_path_in_root,
    ensure_gutch_repo_layout, write_gutch_repo_manifest,
};
pub use repo::remote::{
    PullConflictPolicy, RemoteArtifactSource, RemoteBundleSource, RemoteRepository,
    RemoteRepositoryCommandProvider, RemoteRepositoryProvider, RemoteRepositoryReader,
    RemoteRepositorySummary, RemoteRepositoryWriter, RemoteSnapshot, RemoteSnapshotSources,
    RepoPullPlan, RepoPullStats, RepoSyncPlan, RepoSyncSnapshotChange, RepoSyncUpStats,
    bind_repo_remote, link_repo, plan_pull_with_provider, plan_sync_up_with_provider, pull_repo,
    sync_up_repo, unlink_repo,
};
pub(crate) use repo::remote::{
    RemoteTagRequirementsResult, inspect_repo_tag_requirements, list_remote_repositories,
};
pub use repo::snapshot_manifests::{
    load_snapshot_manifest, save_snapshot_product_metadata_for_snapshots, snapshot_metadata_path,
    validate_snapshot_product_metadata_for_snapshots,
};
pub(crate) use repo::snapshot_manifests::{
    serialize_snapshot_metadata_for_remote, serialize_snapshot_product_metadata_for_remote,
    snapshot_blueprint_path, write_snapshot_manifest_artifacts,
};
pub use repo::snapshot_sources::{
    SnapshotSourcesCaptureOptions, SnapshotSourcesCapturePlan, SnapshotSourcesCaptureResult,
    SnapshotSourcesRefresh, capture_snapshot_sources, describe_snapshot_sources_capture_plan,
    plan_snapshot_sources_capture, plan_snapshot_sources_capture_with_context,
};
pub(crate) use repo::source_identity::compute_snapshot_id_from_entries;
pub use repo::tag_requirements::{TagRequirement, TagRequirementStatus, compare_tag_requirements};
pub use repo::updates::{update_gutch_repo_name, update_gutch_repo_schema};
pub use repo::verify::{RepoVerifyResult, SnapshotVerifyResult, verify_gutch_repo_artifacts};
pub use runtime::RuntimeResourceHints;
pub(crate) use runtime::{RuntimePlan, build_thread_pool, plan_build_runtime};
pub(crate) use snapshot::identity::resolve_snapshot_identity_with_inference;
pub(crate) use snapshot::path_guard::validate_manifest_relative_path;
pub use snapshot::path_guard::{ExistingDestinationPathKind, resolve_existing_destination_path};
pub(crate) use snapshot_ops::ReconstructTimings;
pub use snapshot_ops::{
    BundleDownload, BundleUrlProvider, DEFAULT_ADAPTIVE_TRANSFER_MAXIMUM,
    DEFAULT_TRANSFER_CONCURRENCY, ReconstructedCorruptionOperation, ReconstructedCorruptionPlan,
    ReconstructedCorruptionRequest, ReconstructedCorruptionResult, ResolvedSnapshotOperationState,
    SnapshotDownloadAndReconstructRequest, SnapshotDownloadAndReconstructResult,
    SnapshotFetchRequest, SnapshotFetchResult, SnapshotReconstructFromRepoRequest,
    SnapshotReconstructFromRepoResult, SnapshotReconstructResult, SnapshotRepairFromRepoRequest,
    SnapshotRepairFromRepoResult, SnapshotRepairPlanFromRepoRequest,
    SnapshotRepairPlanFromRepoResult, SnapshotRepairResult, SnapshotVerifyFromRepoRequest,
    SnapshotVerifyFromRepoResult, TransferConcurrencyPolicy, TransferStats,
    corrupt_reconstructed_snapshot, download_and_reconstruct_snapshot, fetch_snapshot,
    fetch_snapshot_for_required_bundle_ids, plan_repair_snapshot_from_repo,
    reconstruct_snapshot_from_repo, repair_snapshot_from_repo, resolve_snapshot_state,
    verify_installed_snapshot,
};
pub use transport::read_bounded_response;
pub(crate) use verify::VerifyTimings;
pub(crate) use verify::estimate_build;
pub use verify::{RepairClassification, RepairIssue, RepairIssueKind, VerifyMode};

#[cfg(test)]
mod tests;
