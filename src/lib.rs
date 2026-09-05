//! Gutch artifact engine.
//!
//! Cross-crate callers use [`api`]. All other modules are implementation
//! details of the engine or its command adapter.

#![warn(clippy::all)]
#![warn(rust_2024_compatibility)]

extern crate self as gutch;

pub mod api;
mod artifact;
mod build;
mod command_api;
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

pub(crate) use artifact::hash::ChunkHash;
pub(crate) use snapshot_ops::snapshot_manifest_from_download;

pub(crate) use build::pipeline::build_from_directory;
pub(crate) use build::pipeline::build_with_config_timed_on_pool;
pub(crate) use build::types::BuildTimings;
pub(crate) use build::types::ExistingChunkLocation;
pub(crate) use build::types::{BuildConfig, BuiltBundle, BuiltLooseChunk};
pub(crate) use error::{Error, Result};
pub(crate) use profile::file_trace_args;
pub(crate) use profile::{GutchProfileSpan, GutchProfiler};
pub(crate) use progress::emit_progress;
pub(crate) use progress::{
    GutchOperation, GutchOperationControl, GutchProgressEvent, GutchProgressPhase,
    GutchProgressSink,
};
pub(crate) use repo::capture_plan::{
    RepoSnapshotCaptureRequest, SnapshotCaptureKind, SnapshotCapturePlan,
    capture_repo_snapshot_with_plan, delete_repo_snapshot_and_cleanup,
    describe_snapshot_capture_plan, plan_snapshot_capture_with_control,
};
pub(crate) use repo::capture_plan::{
    capture_repo_snapshot_with_existing_store_state, delete_repo_snapshots,
    delete_repo_snapshots_with_matching_tags,
};
pub(crate) use repo::check::{RepoCheckSnapshotStatus, check_gutch_repo};
pub(crate) use repo::chunk_index::{
    add_built_bundles_to_chunk_index, add_built_loose_chunks_to_chunk_index, load_chunk_index,
    write_chunk_index,
};
pub(crate) use repo::cleanup::{cleanup_orphan_bundles_in_repo, remove_snapshot_from_repo};
pub(crate) use repo::init_policy::{
    RepoInitAssessment, RepoInitOptions, RepoInitPathKind, assess_repo_init_path,
    initialize_gutch_repo_with_options,
};
pub(crate) use repo::paths::{GutchRepoPaths, load_gutch_repo_manifest};
pub(crate) use repo::paths::{
    bundle_id_from_bundle_path, bundle_path, bundle_path_in_root, chunk_path, chunk_path_in_root,
    ensure_gutch_repo_layout, write_gutch_repo_manifest,
};
pub(crate) use repo::remote::{
    PullConflictPolicy, RemoteRepositoryCommandProvider, RemoteRepositorySummary, RepoPullPlan,
    RepoSyncPlan, bind_repo_remote, link_repo, plan_pull_with_provider, plan_sync_up_with_provider,
    pull_repo, sync_up_repo, unlink_repo,
};
pub(crate) use repo::remote::{
    RemoteTagReferencesResult, inspect_repo_tag_references, list_remote_repositories,
};
pub(crate) use repo::snapshot_manifests::{load_snapshot_manifest, snapshot_metadata_path};
pub(crate) use repo::snapshot_manifests::{
    serialize_snapshot_metadata_for_remote, serialize_snapshot_product_metadata_for_remote,
    snapshot_blueprint_path, write_snapshot_manifest_artifacts,
};
pub(crate) use repo::snapshot_sources::{
    SnapshotSourcesCaptureOptions, SnapshotSourcesCapturePlan, SnapshotSourcesCaptureResult,
    capture_snapshot_sources, describe_snapshot_sources_capture_plan,
    plan_snapshot_sources_capture,
};
pub(crate) use repo::source_identity::compute_snapshot_id_from_entries;
pub(crate) use repo::tag_references::{TagReference, TagReferenceStatus, advisory_tag_references};
pub(crate) use repo::updates::update_gutch_repo_name;
pub(crate) use repo::verify::verify_gutch_repo_artifacts;
pub(crate) use runtime::RuntimeResourceHints;
pub(crate) use runtime::{RuntimePlan, build_thread_pool, plan_build_runtime};
pub(crate) use snapshot::path_guard::validate_manifest_relative_path;
pub(crate) use snapshot_ops::ReconstructTimings;
pub(crate) use snapshot_ops::{
    BundleDownload, BundleUrlProvider, DEFAULT_ADAPTIVE_TRANSFER_MAXIMUM,
    DEFAULT_TRANSFER_CONCURRENCY, ReconstructedCorruptionPlan, ReconstructedCorruptionRequest,
    SnapshotDownloadAndReconstructRequest, SnapshotDownloadAndReconstructResult,
    SnapshotFetchRequest, SnapshotReconstructFromRepoRequest, SnapshotRepairFromRepoRequest,
    SnapshotVerifyFromRepoRequest, TransferConcurrencyPolicy, TransferStats,
    corrupt_reconstructed_snapshot, download_and_reconstruct_snapshot, fetch_snapshot,
    reconstruct_snapshot_from_repo, repair_snapshot_from_repo, verify_installed_snapshot,
};
pub(crate) use transport::read_bounded_response;
pub(crate) use verify::VerifyMode;
pub(crate) use verify::VerifyTimings;
pub(crate) use verify::estimate_build;

#[cfg(test)]
pub(crate) use manifest::launch::{ResolvedLaunchAction, ResolvedLaunchConfiguration};
#[cfg(test)]
pub(crate) use progress::GutchCancellationHook;
#[cfg(test)]
pub(crate) use snapshot_ops::{SnapshotRepairPlanFromRepoRequest, plan_repair_snapshot_from_repo};
#[cfg(test)]
pub(crate) use verify::RepairClassification;

#[cfg(test)]
mod tests;
