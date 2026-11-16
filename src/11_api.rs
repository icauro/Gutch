//! Supported public Gutch API.
//!
//! The crate root intentionally exposes only this module. Internal modules and
//! implementation paths are not part of the cross-crate contract.

// Shared values and operation control
pub use crate::artifact::hash::ChunkHash;
pub use crate::error::Error;
pub use crate::error::Result;
pub use crate::error::metadata_conflict::MetadataFieldDifference;
pub use crate::error::path_validation::{PathValidationFailure, PathViolation, ViolationCode};
pub use crate::error::promotion_diagnostics::{
    DiagnosticIoError, DirectoryPromotionDiagnostic, PromotionPathObservation, PromotionPathState,
    PromotionPhase, PromotionRecoveryAttempt, PromotionRecoveryOperation,
};
pub use crate::profiling::{ProfileMode, ProfileSpanSummary, ProfileSummary, Profiler};
pub use crate::progress::CancellationHook;
pub use crate::progress::OperationControl;
pub use crate::progress::OperationKind;
pub use crate::progress::OperationWarning;
pub use crate::progress::ProgressEvent;
pub use crate::progress::ProgressPhase;
pub use crate::progress::ProgressSink;
pub use crate::progress::WarningSink;
pub use crate::progress::{LiveFeedback, Throughput};
pub use crate::runtime::RuntimeResourceHints;

// Repository and snapshot metadata
pub use crate::artifact::profiles::ArtifactStorageMode;
pub use crate::artifact::profiles::BundleSizingPreset;
pub use crate::artifact::profiles::ChunkSizingPreset;
pub use crate::artifact::profiles::Chunker;
pub use crate::artifact::profiles::CompressionKind;
pub use crate::manifest::ChunkRef;
pub use crate::manifest::DirectoryManifest;
pub use crate::manifest::Estimate;
pub use crate::manifest::ExternalProductDependencyRequirement;
pub use crate::manifest::FileManifest;
pub use crate::manifest::SnapshotDirectRuntimeMetadata;
pub use crate::manifest::SnapshotEntry;
pub use crate::manifest::SnapshotHostVariant;
pub use crate::manifest::SnapshotHostVariantRuntimeMetadata;
pub use crate::manifest::SnapshotLaunchAction;
pub use crate::manifest::SnapshotManifest;
pub use crate::manifest::SnapshotProductMetadata;
pub use crate::manifest::SnapshotRuntimeMetadata;
pub use crate::manifest::SnapshotTagSet;
pub use crate::manifest::SymlinkManifest;
pub use crate::manifest::constraints::{FilesystemTarget, RepositoryConstraints};
pub use crate::manifest::constraints::{
    SnapshotTagConstraints, SnapshotTagRule, SnapshotTagValueFormat,
};
pub use crate::manifest::launch::ResolvedLaunchAction;
pub use crate::manifest::launch::ResolvedLaunchConfiguration;
pub use crate::manifest::repo_manifest::ArtifactProfile;
pub use crate::manifest::repo_manifest::RepoManifest;
pub use crate::manifest::{
    DocumentType, DocumentVersion, REPOSITORY_FORMAT_VERSION, RepositoryFormat, VersionedDocument,
};
pub use crate::path_compare::PathComparisonResult;
pub use crate::path_compare::compare_paths;
pub use crate::snapshot::diff::{SnapshotChangeKind, SnapshotDiff, SnapshotEntryChange};

// Capture
pub use crate::capture::contracts::SnapshotCaptureOutcome;
pub use crate::capture::contracts::SnapshotCapturePlan;
pub use crate::capture::contracts::SnapshotCaptureRequest;
pub use crate::capture::contracts::SnapshotCaptureResult;
pub use crate::capture::estimate::SnapshotEstimateResult;
pub use crate::capture::estimate::estimate_snapshot;
pub use crate::capture::execute::capture_snapshot;
pub use crate::capture::plan::describe_snapshot_capture_plan;
pub use crate::capture::plan::plan_snapshot_capture;
pub use crate::capture::sources::ResolvedCaptureSource;
pub use crate::capture::sources::capture_snapshots;
pub use crate::capture::sources::config::initialize_source_config;
pub use crate::capture::sources::configured::SnapshotSourcesCapturePlan;
pub use crate::capture::sources::configured::SnapshotSourcesCaptureResult;
pub use crate::capture::sources::configured::capture_snapshot_sources;
pub use crate::capture::sources::configured::describe_snapshot_sources_capture_plan;
pub use crate::capture::sources::configured::plan_snapshot_sources_capture;
pub use crate::capture::sources::configured::plan_snapshot_sources_capture_with_context;
pub use crate::capture::sources::discovery::SourceSnapshotCandidate;
pub use crate::capture::sources::discovery::SourceSnapshotDisposition;
pub use crate::capture::sources::discovery::discover_source_snapshots;
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
pub use crate::installation::contracts::InstallationCorruptionOperation;
pub use crate::installation::contracts::InstallationCorruptionResult;
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
pub use crate::installation::verify_installation;

// Repository operations and remote providers
pub use crate::artifact::bundle::BundleTableEntry;
pub use crate::repo::artifacts::inspect_bundle;
pub use crate::repo::init_policy::RepoInitAssessment;
pub use crate::repo::init_policy::RepoInitOptions;
pub use crate::repo::init_policy::RepoInitPathKind;
pub use crate::repo::init_policy::RepoInitRisk;
pub use crate::repo::init_policy::assess_repo_init_path;
pub use crate::repo::init_policy::initialize_repo;
pub use crate::repo::paths::RepoPaths;
pub use crate::repo::read::{load_gutch_repo_manifest, load_snapshot_manifest};
pub use crate::repo::recovery::IndexRebuildResult;
pub use crate::repo::recovery::rebuild_index;
pub use crate::repo::snapshot_manifests::diff_repo_snapshots;
pub use crate::repo::snapshot_manifests::save_snapshot_product_metadata_for_snapshots;
pub use crate::repo::snapshot_manifests::validate_snapshot_product_metadata_for_snapshots;
pub use crate::repo::snapshots::RetagSnapshotResult;
pub use crate::repo::snapshots::delete_repo_snapshot_and_cleanup;
pub use crate::repo::snapshots::retag_snapshot_by_id;
pub use crate::repo::snapshots::{plan_snapshot_tag_updates, retag_snapshots};
pub use crate::repo::status::RepoSnapshotStatus;
pub use crate::repo::status::RepoStatus;
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
pub use crate::sync::fetch::fetch_snapshot;
pub use crate::sync::fetch::fetch_snapshot_for_artifact_ids;
pub use crate::sync::fetch_types::SnapshotFetchRequest;
pub use crate::sync::fetch_types::SnapshotFetchResult;
pub use crate::sync::remote::PullConflictPolicy;
pub use crate::sync::remote::RemoteArtifactSource;
pub use crate::sync::remote::RemoteBundleSource;
pub use crate::sync::remote::RemoteRepository;
pub use crate::sync::remote::RemoteRepositoryCommandProvider;
pub use crate::sync::remote::RemoteRepositoryProvider;
pub use crate::sync::remote::RemoteRepositoryReader;
pub use crate::sync::remote::RemoteRepositorySummary;
pub use crate::sync::remote::RemoteRepositoryWriter;
pub use crate::sync::remote::RemoteSnapshot;
pub use crate::sync::remote::RemoteSnapshotSources;
pub use crate::sync::remote::RemoteTagReferencesResult;
pub use crate::sync::remote::RepoPullPlan;
pub use crate::sync::remote::RepoPullStats;
pub use crate::sync::remote::RepoPushPlan;
pub use crate::sync::remote::RepoPushSnapshotChange;
pub use crate::sync::remote::RepoPushStats;
pub use crate::sync::remote::bind_remote_repo;
pub use crate::sync::remote::create_remote_repo;
pub use crate::sync::remote::inspect_repo_tag_references;
pub use crate::sync::remote::list_remote_repositories;
pub use crate::sync::remote::plan_pull_with_provider;
pub use crate::sync::remote::plan_push_with_provider;
pub use crate::sync::remote::pull_repo;
pub use crate::sync::remote::push_repo;
pub use crate::sync::remote::unbind_remote_repo;

// Artifact transfer
pub use crate::transfer::types::ArtifactDownload;
pub use crate::transfer::types::ArtifactUrlProvider;
pub use crate::transfer::types::TransferConcurrencyPolicy;
pub use crate::transfer::types::TransferStats;

// CLI hosting
pub use crate::cli::requires_remote_provider;
pub use crate::cli::run_from_args;
pub use crate::cli::run_from_args_with_remote_provider;
pub use crate::cli::run_from_args_with_remote_provider_named;
