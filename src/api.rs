//! Supported public Gutch API.
//!
//! The crate root intentionally exposes only this module. Internal modules and
//! implementation paths are not part of the cross-crate contract.

// Shared values and operation control
pub use crate::artifact::hash::ChunkHash;
pub use crate::artifact::hash::hash_chunk;
pub use crate::artifact::hash::hash_hex;
pub use crate::error::Error;
pub use crate::error::Result;
pub use crate::filesystem::constraints::{PathValidationFailure, PathViolation};
pub use crate::filesystem::paths::ExistingDestinationPathKind;
pub use crate::filesystem::paths::resolve_existing_destination_path;
pub use crate::profile::{
    GutchProfileMode, GutchProfileSpanSummary, GutchProfileSummary, GutchProfiler,
};
pub use crate::progress::GutchCancellationHook;
pub use crate::progress::GutchOperation;
pub use crate::progress::GutchOperationControl;
pub use crate::progress::GutchProgressEvent;
pub use crate::progress::GutchProgressPhase;
pub use crate::progress::GutchProgressSink;
pub use crate::progress::{GutchLiveFeedback, GutchThroughput};
pub use crate::runtime::RuntimeResourceHints;

// Repository and snapshot metadata
pub use crate::manifest::ArtifactStorageMode;
pub use crate::manifest::BundleSizingPreset;
pub use crate::manifest::ChunkRef;
pub use crate::manifest::ChunkSizingPreset;
pub use crate::manifest::Chunker;
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
pub use crate::manifest::SnapshotTagSet;
pub use crate::manifest::StructuredVersion;
pub use crate::manifest::SymlinkManifest;
pub use crate::manifest::constraints::{FilesystemTarget, RepositoryConstraints};
pub use crate::manifest::gutch_repo::ArtifactProfile;
pub use crate::manifest::gutch_repo::GutchRepoManifest;
pub use crate::manifest::launch::ResolvedLaunchAction;
pub use crate::manifest::launch::ResolvedLaunchConfiguration;
pub use crate::path_compare::PathComparisonResult;
pub use crate::path_compare::compare_paths;
pub use crate::snapshot::diff::{SnapshotChangeKind, SnapshotDiff, SnapshotEntryChange};

// Capture
pub use crate::capture::configured_sources::SnapshotSourcesCapturePlan;
pub use crate::capture::configured_sources::SnapshotSourcesCaptureResult;
pub use crate::capture::configured_sources::capture_snapshot_sources;
pub use crate::capture::configured_sources::describe_snapshot_sources_capture_plan;
pub use crate::capture::configured_sources::plan_snapshot_sources_capture;
pub use crate::capture::configured_sources::plan_snapshot_sources_capture_with_context;
pub use crate::capture::contracts::MetadataFieldDifference;
pub use crate::capture::contracts::SnapshotCaptureOutcome;
pub use crate::capture::contracts::SnapshotCapturePlan;
pub use crate::capture::contracts::SnapshotCaptureRequest;
pub use crate::capture::contracts::SnapshotCaptureResult;
pub use crate::capture::estimate::SnapshotEstimateResult;
pub use crate::capture::estimate::estimate_snapshot;
pub use crate::capture::execute::capture_snapshot;
pub use crate::capture::plan::describe_snapshot_capture_plan;
pub use crate::capture::plan::plan_snapshot_capture;
pub use crate::capture::plan::plan_snapshot_capture_with_control;
pub use crate::capture::sources::ResolvedCaptureSource;
pub use crate::capture::sources::capture_snapshots;
pub use crate::capture::sources::plan_snapshot_captures;
pub use crate::capture::sources::resolve_capture_sources;

// Installation
pub use crate::installation::InstallationCorruptionPlan;
pub use crate::installation::InstallationCorruptionRequest;
pub use crate::installation::InstallationReconstructResult;
pub use crate::installation::InstallationRepairPlanRequest;
pub use crate::installation::InstallationRepairPlanResult;
pub use crate::installation::InstallationRepairRequest;
pub use crate::installation::InstallationRepairResult;
pub use crate::installation::InstallationRequest;
pub use crate::installation::InstallationVerifyRequest;
pub use crate::installation::InstallationVerifyResult;
pub use crate::installation::ResolvedSnapshotOperationState;
pub use crate::installation::SnapshotReconstructResult;
pub use crate::installation::SnapshotRepairResult;
pub use crate::installation::corrupt_installation;
pub use crate::installation::fetch::SnapshotFetchAndReconstructRequest;
pub use crate::installation::fetch::SnapshotFetchAndReconstructResult;
pub use crate::installation::fetch::fetch_and_reconstruct_snapshot;
pub use crate::installation::inspect::RepairClassification;
pub use crate::installation::inspect::RepairIssue;
pub use crate::installation::inspect::RepairIssueKind;
pub use crate::installation::inspect::VerifyMode;
pub use crate::installation::install::{
    ExtraFilePolicy, InstallationPublication, InstallationResult, install_snapshot_from_repo,
};
pub use crate::installation::plan_installation_repair;
pub use crate::installation::preview::{
    InstallationAction, InstallationInspection, InstallationPlanEvent, InstallationPlanSummary,
    visit_installation_plan,
};
pub use crate::installation::reconstruct_snapshot;
pub use crate::installation::repair_installation;
pub use crate::installation::resolve_snapshot_state;
pub use crate::installation::types::InstallationCorruptionOperation;
pub use crate::installation::types::InstallationCorruptionResult;
pub use crate::installation::verify_installation;

// Repository operations and remote providers
pub use crate::repo::artifacts::inspect_bundle;
pub use crate::repo::fetch::fetch_snapshot;
pub use crate::repo::fetch::fetch_snapshot_for_artifact_ids;
pub use crate::repo::fetch_types::SnapshotFetchRequest;
pub use crate::repo::fetch_types::SnapshotFetchResult;
pub use crate::repo::init_policy::RepoInitAssessment;
pub use crate::repo::init_policy::RepoInitOptions;
pub use crate::repo::init_policy::RepoInitPathKind;
pub use crate::repo::init_policy::RepoInitRisk;
pub use crate::repo::init_policy::assess_repo_init_path;
pub use crate::repo::init_policy::initialize_repo;
pub use crate::repo::paths::GutchRepoPaths;
pub use crate::repo::recovery::IndexRebuildResult;
pub use crate::repo::recovery::rebuild_index;
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
pub use crate::repo::remote::RepoPushPlan;
pub use crate::repo::remote::RepoPushSnapshotChange;
pub use crate::repo::remote::RepoPushStats;
pub use crate::repo::remote::bind_remote_repo;
pub use crate::repo::remote::create_remote_repo;
pub use crate::repo::remote::inspect_repo_tag_references;
pub use crate::repo::remote::list_remote_repositories;
pub use crate::repo::remote::plan_pull_with_provider;
pub use crate::repo::remote::plan_push_with_provider;
pub use crate::repo::remote::pull_repo;
pub use crate::repo::remote::push_repo;
pub use crate::repo::remote::unbind_remote_repo;
pub use crate::repo::snapshot_manifests::diff_repo_snapshots;
pub use crate::repo::snapshot_manifests::save_snapshot_product_metadata_for_snapshots;
pub use crate::repo::snapshot_manifests::snapshot_metadata_path;
pub use crate::repo::snapshot_manifests::validate_snapshot_product_metadata_for_snapshots;
pub use crate::repo::snapshots::RetagSnapshotResult;
pub use crate::repo::snapshots::delete_repo_snapshot_and_cleanup;
pub use crate::repo::snapshots::retag_snapshot_by_id;
pub use crate::repo::status::RepoSnapshotStatus;
pub use crate::repo::status::RepoStatus;
pub use crate::repo::status::SourceSnapshotOperationCandidate;
pub use crate::repo::status::SourceSnapshotOperationDisposition;
pub use crate::repo::status::discover_source_snapshot_operations;
pub use crate::repo::status::read_repo_status;
pub use crate::repo::tag_references::TagReference;
pub use crate::repo::tag_references::TagReferenceStatus;
pub use crate::repo::tag_references::advisory_tag_references;
pub use crate::repo::updates::rename_repo;
pub use crate::repo::updates::{
    plan_repo_rename, plan_repository_constraints, read_repository_constraints,
    update_repository_constraints,
};
pub use crate::repo::verify::SnapshotArtifactVerifyResult;
pub use crate::repo::verify::verify_gutch_repo_artifacts;
pub use crate::repo::verify::{RepoArtifactVerification, RepoArtifactVerifyResult};

// Artifact transfer
pub use crate::transfer::http::read_bounded_response;
pub use crate::transfer::types::ArtifactDownload;
pub use crate::transfer::types::ArtifactUrlProvider;
pub use crate::transfer::types::TransferConcurrencyPolicy;
pub use crate::transfer::types::TransferStats;

// CLI hosting and local configuration
pub use crate::cli::requires_remote_provider;
pub use crate::cli::run_from_args;
pub use crate::cli::run_from_args_with_remote_provider;
pub use crate::cli::run_from_args_with_remote_provider_named;
pub use crate::config::GutchConfig;
pub use crate::config::TrackedRepo;
pub use crate::config::load_config;
pub use crate::config::save_config;

pub use crate::repo::read::{load_gutch_repo_manifest, load_snapshot_manifest};

pub use crate::snapshot::source_config::initialize_source_config;

pub use crate::manifest::constraints::{
    SnapshotTagConstraints, SnapshotTagRule, SnapshotTagValueFormat,
};

pub use crate::repo::snapshots::{plan_snapshot_tag_updates, retag_snapshots};
