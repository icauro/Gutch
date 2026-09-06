//! Supported public Gutch API.
//!
//! The crate root intentionally exposes only this module. Internal modules and
//! implementation paths are not part of the cross-crate contract.

pub use crate::artifact::bundle::inspect_bundle;
pub use crate::artifact::hash::{ChunkHash, hash_chunk, hash_hex};
pub use crate::build::estimate::{SnapshotEstimateResult, estimate_snapshot};
pub use crate::command_api::{
    requires_remote_provider, run_from_args, run_from_args_with_remote_provider,
    run_from_args_with_remote_provider_named,
};
pub use crate::config::{GutchConfig, TrackedRepo, load_config, save_config};
pub use crate::error::{Error, Result};
pub use crate::manifest::gutch_repo::{ArtifactProfile, GutchRepoManifest};
pub use crate::manifest::launch::{ResolvedLaunchAction, ResolvedLaunchConfiguration};
pub use crate::manifest::{
    ArtifactSizingPreset, ArtifactStorageMode, BundleSizingPreset, BundleTableEntry,
    CONTENT_METADATA_FORMAT_VERSION, ChunkRef, CompressionKind, DirectoryManifest, Estimate,
    ExternalProductDependencyRequirement, FileManifest, PUBLIC_METADATA_FORMAT_VERSION,
    SnapshotDirectRuntimeMetadata, SnapshotEntry, SnapshotHostVariant,
    SnapshotHostVariantRuntimeMetadata, SnapshotLaunchAction, SnapshotManifest,
    SnapshotProductMetadata, SnapshotRuntimeMetadata, SnapshotTagKind, SnapshotTagSet,
    StructuredVersion, SymlinkManifest,
};
pub use crate::path_compare::{PathComparisonResult, compare_paths};
pub use crate::progress::{
    GutchCancellationHook, GutchOperation, GutchOperationControl, GutchProgressEvent,
    GutchProgressPhase, GutchProgressSink,
};
pub use crate::repo::capture_plan::{
    MetadataFieldDifference, RepoSnapshotCaptureRequest, RepoSnapshotCaptureResult,
    RetagSnapshotResult, SnapshotCaptureOutcome, SnapshotCapturePlan, capture_repo_snapshot,
    delete_repo_snapshot_and_cleanup, describe_snapshot_capture_plan, plan_snapshot_capture,
    plan_snapshot_capture_with_control, retag_snapshot_by_id,
};
pub use crate::repo::capture_sources::{
    ResolvedCaptureSource, capture_repo_snapshots, plan_repo_snapshot_captures,
    resolve_capture_sources,
};
pub use crate::repo::check::{
    RepoCheckSnapshotStatus, RepoCheckStatus, SourceSnapshotOperationCandidate,
    SourceSnapshotOperationDisposition, check_gutch_repo, discover_source_snapshot_operations,
};
pub use crate::repo::init_policy::{
    RepoInitAssessment, RepoInitOptions, RepoInitPathKind, RepoInitRisk, assess_repo_init_path,
    initialize_gutch_repo_with_options,
};
pub use crate::repo::paths::{GutchRepoPaths, load_gutch_repo_manifest};
pub use crate::repo::remote::{
    PullConflictPolicy, RemoteArtifactSource, RemoteBundleSource, RemoteRepository,
    RemoteRepositoryCommandProvider, RemoteRepositoryProvider, RemoteRepositoryReader,
    RemoteRepositorySummary, RemoteRepositoryWriter, RemoteSnapshot, RemoteSnapshotSources,
    RemoteTagReferencesResult, RepoPullPlan, RepoPullStats, RepoSyncPlan, RepoSyncSnapshotChange,
    RepoSyncUpStats, bind_repo_remote, inspect_repo_tag_references, link_repo,
    list_remote_repositories, plan_pull_with_provider, plan_sync_up_with_provider, pull_repo,
    sync_up_repo, unlink_repo,
};
pub use crate::repo::snapshot_manifests::{
    load_snapshot_manifest, save_snapshot_product_metadata_for_snapshots, snapshot_metadata_path,
    validate_snapshot_product_metadata_for_snapshots,
};
pub use crate::repo::snapshot_sources::{
    SnapshotSourcesCapturePlan, SnapshotSourcesCaptureResult, capture_snapshot_sources,
    describe_snapshot_sources_capture_plan, plan_snapshot_sources_capture,
    plan_snapshot_sources_capture_with_context,
};
pub use crate::repo::tag_references::{TagReference, TagReferenceStatus, advisory_tag_references};
pub use crate::repo::updates::update_gutch_repo_name;
pub use crate::repo::verify::{
    RepoVerifyResult, SnapshotVerifyResult, verify_gutch_repo_artifacts,
};
pub use crate::runtime::RuntimeResourceHints;
pub use crate::snapshot::path_guard::{
    ExistingDestinationPathKind, resolve_existing_destination_path,
};
pub use crate::snapshot_ops::types::{
    ReconstructedCorruptionOperation, ReconstructedCorruptionResult,
};
pub use crate::snapshot_ops::{
    BundleDownload, BundleUrlProvider, ReconstructedCorruptionPlan, ReconstructedCorruptionRequest,
    ResolvedSnapshotOperationState, SnapshotDownloadAndReconstructRequest,
    SnapshotDownloadAndReconstructResult, SnapshotFetchRequest, SnapshotFetchResult,
    SnapshotReconstructFromRepoRequest, SnapshotReconstructFromRepoResult,
    SnapshotReconstructResult, SnapshotRepairFromRepoRequest, SnapshotRepairFromRepoResult,
    SnapshotRepairPlanFromRepoRequest, SnapshotRepairPlanFromRepoResult, SnapshotRepairResult,
    SnapshotVerifyFromRepoRequest, SnapshotVerifyFromRepoResult, TransferConcurrencyPolicy,
    TransferStats, corrupt_reconstructed_snapshot, download_and_reconstruct_snapshot,
    fetch_snapshot, fetch_snapshot_for_required_bundle_ids, plan_repair_snapshot_from_repo,
    reconstruct_snapshot_from_repo, repair_snapshot_from_repo, resolve_snapshot_state,
    verify_installed_snapshot,
};
pub use crate::transport::read_bounded_response;
pub use crate::verify::{RepairClassification, RepairIssue, RepairIssueKind, VerifyMode};
