//! Supported public Gutch API.
//!
//! The crate root intentionally exposes only this module. Internal modules and
//! implementation paths are not part of the cross-crate contract.

pub use crate::artifact::bundle::BundleTableEntry;
pub use crate::artifact::hash::ChunkHash;
pub use crate::artifact::hash::hash_chunk;
pub use crate::artifact::hash::hash_hex;
pub use crate::capture::configured_sources::SnapshotSourcesCapturePlan;
pub use crate::capture::configured_sources::SnapshotSourcesCaptureResult;
pub use crate::capture::configured_sources::capture_snapshot_sources;
pub use crate::capture::configured_sources::describe_snapshot_sources_capture_plan;
pub use crate::capture::configured_sources::plan_snapshot_sources_capture;
pub use crate::capture::configured_sources::plan_snapshot_sources_capture_with_context;
pub use crate::capture::estimate::SnapshotEstimateResult;
pub use crate::capture::estimate::estimate_snapshot;
pub use crate::capture::execute::MetadataFieldDifference;
pub use crate::capture::execute::RepoSnapshotCaptureRequest;
pub use crate::capture::execute::RepoSnapshotCaptureResult;
pub use crate::capture::execute::SnapshotCaptureOutcome;
pub use crate::capture::execute::SnapshotCapturePlan;
pub use crate::capture::execute::capture_repo_snapshot;
pub use crate::capture::plan::describe_snapshot_capture_plan;
pub use crate::capture::plan::plan_snapshot_capture;
pub use crate::capture::plan::plan_snapshot_capture_with_control;
pub use crate::capture::sources::ResolvedCaptureSource;
pub use crate::capture::sources::capture_repo_snapshots;
pub use crate::capture::sources::plan_repo_snapshot_captures;
pub use crate::capture::sources::resolve_capture_sources;
pub use crate::cli::requires_remote_provider;
pub use crate::cli::run_from_args;
pub use crate::cli::run_from_args_with_remote_provider;
pub use crate::cli::run_from_args_with_remote_provider_named;
pub use crate::config::GutchConfig;
pub use crate::config::TrackedRepo;
pub use crate::config::load_config;
pub use crate::config::save_config;
pub use crate::error::Error;
pub use crate::error::Result;
pub use crate::filesystem::paths::ExistingDestinationPathKind;
pub use crate::filesystem::paths::resolve_existing_destination_path;
pub use crate::installation::ReconstructedCorruptionPlan;
pub use crate::installation::ReconstructedCorruptionRequest;
pub use crate::installation::ResolvedSnapshotOperationState;
pub use crate::installation::SnapshotReconstructFromRepoRequest;
pub use crate::installation::SnapshotReconstructFromRepoResult;
pub use crate::installation::SnapshotReconstructResult;
pub use crate::installation::SnapshotRepairFromRepoRequest;
pub use crate::installation::SnapshotRepairFromRepoResult;
pub use crate::installation::SnapshotRepairPlanFromRepoRequest;
pub use crate::installation::SnapshotRepairPlanFromRepoResult;
pub use crate::installation::SnapshotRepairResult;
pub use crate::installation::SnapshotVerifyFromRepoRequest;
pub use crate::installation::SnapshotVerifyFromRepoResult;
pub use crate::installation::corrupt_reconstructed_snapshot;
pub use crate::installation::inspect::RepairClassification;
pub use crate::installation::inspect::RepairIssue;
pub use crate::installation::inspect::RepairIssueKind;
pub use crate::installation::inspect::VerifyMode;
pub use crate::installation::plan_repair_snapshot_from_repo;
pub use crate::installation::reconstruct_snapshot_from_repo;
pub use crate::installation::repair_snapshot_from_repo;
pub use crate::installation::resolve_snapshot_state;
pub use crate::installation::types::ReconstructedCorruptionOperation;
pub use crate::installation::types::ReconstructedCorruptionResult;
pub use crate::installation::verify_installed_snapshot;
pub use crate::manifest::ArtifactSizingPreset;
pub use crate::manifest::ArtifactStorageMode;
pub use crate::manifest::BundleSizingPreset;
pub use crate::manifest::ChunkRef;
pub use crate::manifest::CompressionKind;
pub use crate::manifest::DirectoryManifest;
pub use crate::manifest::Estimate;
pub use crate::manifest::ExternalProductDependencyRequirement;
pub use crate::manifest::FileManifest;
pub use crate::manifest::PUBLIC_METADATA_FORMAT_VERSION;
pub use crate::manifest::SnapshotDirectRuntimeMetadata;
pub use crate::manifest::SnapshotEntry;
pub use crate::manifest::SnapshotHostVariant;
pub use crate::manifest::SnapshotHostVariantRuntimeMetadata;
pub use crate::manifest::SnapshotLaunchAction;
pub use crate::manifest::SnapshotManifest;
pub use crate::manifest::SnapshotProductMetadata;
pub use crate::manifest::SnapshotRuntimeMetadata;
pub use crate::manifest::SnapshotTagKind;
pub use crate::manifest::SnapshotTagSet;
pub use crate::manifest::StructuredVersion;
pub use crate::manifest::SymlinkManifest;
pub use crate::manifest::gutch_repo::ArtifactProfile;
pub use crate::manifest::gutch_repo::GutchRepoManifest;
pub use crate::manifest::launch::ResolvedLaunchAction;
pub use crate::manifest::launch::ResolvedLaunchConfiguration;
pub use crate::path_compare::PathComparisonResult;
pub use crate::path_compare::compare_paths;
pub use crate::progress::GutchCancellationHook;
pub use crate::progress::GutchOperation;
pub use crate::progress::GutchOperationControl;
pub use crate::progress::GutchProgressEvent;
pub use crate::progress::GutchProgressPhase;
pub use crate::progress::GutchProgressSink;
pub use crate::repo::artifacts::inspect_bundle;
pub use crate::repo::check::RepoCheckSnapshotStatus;
pub use crate::repo::check::RepoCheckStatus;
pub use crate::repo::check::SourceSnapshotOperationCandidate;
pub use crate::repo::check::SourceSnapshotOperationDisposition;
pub use crate::repo::check::check_gutch_repo;
pub use crate::repo::check::discover_source_snapshot_operations;
pub use crate::repo::init_policy::RepoInitAssessment;
pub use crate::repo::init_policy::RepoInitOptions;
pub use crate::repo::init_policy::RepoInitPathKind;
pub use crate::repo::init_policy::RepoInitRisk;
pub use crate::repo::init_policy::assess_repo_init_path;
pub use crate::repo::init_policy::initialize_gutch_repo_with_options;
pub use crate::repo::paths::GutchRepoPaths;
pub use crate::repo::recovery::ChunkMapRegenerationResult;
pub use crate::repo::recovery::regenerate_chunk_map;
pub use crate::repo::remote::PullConflictPolicy;
pub use crate::repo::remote::RemoteArtifactSource;
pub use crate::repo::remote::RemoteBundleSource;
pub use crate::repo::remote::RemoteRepository;
pub use crate::repo::remote::RemoteRepositoryCommandProvider;
pub use crate::repo::remote::RemoteRepositoryProvider;
pub use crate::repo::remote::RemoteRepositoryReader;
pub use crate::repo::remote::RemoteRepositorySummary;
pub use crate::repo::remote::RemoteRepositoryWriter;
pub use crate::repo::remote::RemoteSnapshot;
pub use crate::repo::remote::RemoteSnapshotSources;
pub use crate::repo::remote::RemoteTagReferencesResult;
pub use crate::repo::remote::RepoPullPlan;
pub use crate::repo::remote::RepoPullStats;
pub use crate::repo::remote::RepoSyncPlan;
pub use crate::repo::remote::RepoSyncSnapshotChange;
pub use crate::repo::remote::RepoSyncUpStats;
pub use crate::repo::remote::bind_repo_remote;
pub use crate::repo::remote::inspect_repo_tag_references;
pub use crate::repo::remote::link_repo;
pub use crate::repo::remote::list_remote_repositories;
pub use crate::repo::remote::plan_pull_with_provider;
pub use crate::repo::remote::plan_sync_up_with_provider;
pub use crate::repo::remote::pull_repo;
pub use crate::repo::remote::sync_up_repo;
pub use crate::repo::remote::unlink_repo;
pub use crate::repo::snapshot_manifests::save_snapshot_product_metadata_for_snapshots;
pub use crate::repo::snapshot_manifests::snapshot_metadata_path;
pub use crate::repo::snapshot_manifests::validate_snapshot_product_metadata_for_snapshots;
pub use crate::repo::snapshots::RetagSnapshotResult;
pub use crate::repo::snapshots::delete_repo_snapshot_and_cleanup;
pub use crate::repo::snapshots::retag_snapshot_by_id;
pub use crate::repo::tag_references::TagReference;
pub use crate::repo::tag_references::TagReferenceStatus;
pub use crate::repo::tag_references::advisory_tag_references;
pub use crate::repo::updates::update_gutch_repo_name;
pub use crate::repo::verify::RepoVerifyResult;
pub use crate::repo::verify::SnapshotVerifyResult;
pub use crate::repo::verify::verify_gutch_repo_artifacts;
pub use crate::runtime::RuntimeResourceHints;
pub use crate::transfer::fetch::download_and_reconstruct_snapshot;
pub use crate::transfer::fetch::fetch_snapshot;
pub use crate::transfer::fetch::fetch_snapshot_for_required_bundle_ids;
pub use crate::transfer::http::read_bounded_response;
pub use crate::transfer::types::BundleDownload;
pub use crate::transfer::types::BundleUrlProvider;
pub use crate::transfer::types::SnapshotDownloadAndReconstructRequest;
pub use crate::transfer::types::SnapshotDownloadAndReconstructResult;
pub use crate::transfer::types::SnapshotFetchRequest;
pub use crate::transfer::types::SnapshotFetchResult;
pub use crate::transfer::types::TransferConcurrencyPolicy;
pub use crate::transfer::types::TransferStats;

/// Load one committed repository metadata document under reader protection.
pub fn load_gutch_repo_manifest(
    root: &std::path::Path,
) -> Result<(GutchRepoPaths, GutchRepoManifest)> {
    crate::repo::coordination::read(root, || crate::repo::paths::load_gutch_repo_manifest(root))
}

/// Load a coherent snapshot metadata/blueprint pair under reader protection.
pub fn load_snapshot_manifest(paths: &GutchRepoPaths, snapshot: &str) -> Result<SnapshotManifest> {
    crate::repo::coordination::read(&paths.repo_root, || {
        crate::repo::snapshot_manifests::load_snapshot_manifest(paths, snapshot)
    })
}

pub use crate::installation::operation::{
    ExtraFilePolicy, InstallationPublication, InstallationResult, install_snapshot_from_repo,
};

pub use crate::installation::preview::{
    InstallationAction, InstallationInspection, InstallationPlanEvent, InstallationPlanSummary,
    visit_installation_plan,
};

pub use crate::profile::{
    GutchProfileMode, GutchProfileSpanSummary, GutchProfileSummary, GutchProfiler,
};

pub use crate::progress::{GutchLiveFeedback, GutchThroughput};
