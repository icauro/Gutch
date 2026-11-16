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
pub use crate::manifest::constraints::{FilesystemTarget, RepoConstraints};
pub use crate::manifest::constraints::{
    SnapshotTagConstraints, SnapshotTagRule, SnapshotTagValueFormat,
};
pub use crate::manifest::launch::ResolvedLaunchAction;
pub use crate::manifest::launch::ResolvedLaunchConfiguration;
pub use crate::manifest::repo_manifest::ArtifactProfile;
pub use crate::manifest::repo_manifest::RepoManifest;
pub use crate::manifest::{
    DocumentType, DocumentVersion, REPO_FORMAT_VERSION, RepoFormat, VersionedDocument,
};
pub use crate::path_compare::PathComparisonResult;
pub use crate::path_compare::compare_paths;
pub use crate::snapshot::diff::{SnapshotChangeKind, SnapshotDiffResult, SnapshotEntryChange};

// Capture
pub use crate::capture::contracts::SnapshotCaptureOutcome;
pub use crate::capture::contracts::SnapshotCapturePlan;
pub use crate::capture::contracts::SnapshotCaptureRequest;
pub use crate::capture::contracts::SnapshotCaptureResult;
pub use crate::capture::measure::measure_snapshot;
pub use crate::capture::measure::{SnapshotMeasureRequest, SnapshotMeasureResult};
pub use crate::capture::sources::ResolvedCaptureSource;
pub use crate::capture::sources::capture_snapshot;
pub use crate::capture::sources::capture_snapshots;
pub use crate::capture::sources::config::init_source_config;
pub use crate::capture::sources::plan_capture_snapshot;
pub use crate::capture::sources::plan_capture_snapshots;
pub use crate::capture::sources::resolve_capture_sources;

// Installation
pub use crate::installation::InstallationCorruptPlan;
pub use crate::installation::InstallationCorruptRequest;
pub use crate::installation::InstallationReconstructRequest;
pub use crate::installation::InstallationReconstructResult;
pub use crate::installation::InstallationRepairPlan;
pub use crate::installation::InstallationRepairRequest;
pub use crate::installation::InstallationRepairResult;
pub use crate::installation::InstallationVerifyRequest;
pub use crate::installation::InstallationVerifyResult;
pub use crate::installation::ResolvedSnapshotOperationState;
pub use crate::installation::contracts::InstallationCorruptOperation;
pub use crate::installation::contracts::InstallationCorruptResult;
pub use crate::installation::corrupt_installation;
pub use crate::installation::fetch::InstallationFetchAndReconstructRequest;
pub use crate::installation::fetch::InstallationFetchAndReconstructResult;
pub use crate::installation::fetch::fetch_and_reconstruct_installation;
pub use crate::installation::inspect::InstallationIssue;
pub use crate::installation::inspect::InstallationIssueKind;
pub use crate::installation::inspect::InstallationIssues;
pub use crate::installation::inspect::InstallationVerifyMode;
pub use crate::installation::install::{ExtraFilePolicy, InstallationPublication};
pub use crate::installation::plan_repair_installation;
pub use crate::installation::preview::{
    InstallationAction, InstallationInspection, InstallationPreviewEvent,
    InstallationPreviewRequest, InstallationPreviewSummary, preview_installation,
};
pub use crate::installation::reconstruct_installation;
pub use crate::installation::repair_installation;
pub use crate::installation::resolve_snapshot_state;
pub use crate::installation::verify_installation;

// Repository operations and remote providers
pub use crate::artifact::bundle::BundleTableEntry;
pub use crate::repo::artifacts::inspect_bundle;
pub use crate::repo::init_policy::RepoInitPathKind;
pub use crate::repo::init_policy::RepoInitPlan;
pub use crate::repo::init_policy::RepoInitRequest;
pub use crate::repo::init_policy::RepoInitResult;
pub use crate::repo::init_policy::RepoInitRisk;
pub use crate::repo::init_policy::init_repo;
pub use crate::repo::init_policy::plan_init_repo;
pub use crate::repo::read::{load_repo_manifest, load_snapshot_manifest};
pub use crate::repo::recovery::{
    RepoIndexRebuildPlan, RepoIndexRebuildRequest, RepoIndexRebuildResult,
};
pub use crate::repo::recovery::{plan_rebuild_repo_index, rebuild_repo_index};
pub use crate::repo::snapshot_manifests::check_snapshot_product_metadata;
pub use crate::repo::snapshot_manifests::set_snapshot_product_metadata;
pub use crate::repo::snapshot_manifests::{SnapshotDiffRequest, diff_snapshots};
pub use crate::repo::snapshots::delete_snapshot;
pub use crate::repo::snapshots::{
    SnapshotDeleteRequest, SnapshotDeleteResult, SnapshotTagsChange, SnapshotTagsReplacePlan,
    SnapshotTagsReplaceRequest, SnapshotTagsReplaceResult,
};
pub use crate::repo::snapshots::{plan_replace_snapshot_tags, replace_snapshot_tags};
pub use crate::repo::status::RepoSnapshotStatus;
pub use crate::repo::status::RepoStatus;
pub use crate::repo::status::repo_status;
pub use crate::repo::tag_references::TagReference;
pub use crate::repo::tag_references::TagReferenceStatus;
pub use crate::repo::updates::rename_repo;
pub use crate::repo::updates::{
    RepoConstraintsSetPlan, RepoConstraintsSetRequest, RepoConstraintsSetResult, RepoRenamePlan,
    RepoRenameRequest, RepoRenameResult,
};
pub use crate::repo::updates::{
    plan_rename_repo, plan_set_repo_constraints, repo_constraints, set_repo_constraints,
};
pub use crate::repo::verify::SnapshotVerifyOutcome;
pub use crate::repo::verify::verify_repo;
pub use crate::repo::verify::{RepoVerifyMode, RepoVerifyRequest, RepoVerifyResult};
pub use crate::sync::fetch::fetch_snapshot;
pub use crate::sync::fetch_types::SnapshotFetchRequest;
pub use crate::sync::fetch_types::SnapshotFetchResult;
pub use crate::sync::remote::PullConflictPolicy;
pub use crate::sync::remote::RemoteArtifactSource;
pub use crate::sync::remote::RemoteArtifactTarget;
pub use crate::sync::remote::RemoteBundleSource;
pub use crate::sync::remote::RemoteRepo;
pub use crate::sync::remote::RemoteRepoBindRequest;
pub use crate::sync::remote::RemoteRepoBindResult;
pub use crate::sync::remote::RemoteRepoCreateRequest;
pub use crate::sync::remote::RemoteRepoCreateResult;
pub use crate::sync::remote::RemoteRepoProvider;
pub use crate::sync::remote::RemoteRepoReader;
pub use crate::sync::remote::RemoteRepoService;
pub use crate::sync::remote::RemoteRepoSummary;
pub use crate::sync::remote::RemoteRepoUnbindRequest;
pub use crate::sync::remote::RemoteRepoUnbindResult;
pub use crate::sync::remote::RemoteRepoWriter;
pub use crate::sync::remote::RemoteSnapshot;
pub use crate::sync::remote::RemoteSnapshotSources;
pub use crate::sync::remote::RemoteTagReferencesResult;
pub use crate::sync::remote::RepoPullPlan;
pub use crate::sync::remote::RepoPullRequest;
pub use crate::sync::remote::RepoPullResult;
pub use crate::sync::remote::RepoPushPlan;
pub use crate::sync::remote::RepoPushRequest;
pub use crate::sync::remote::RepoPushResult;
pub use crate::sync::remote::RepoPushSnapshotChange;
pub use crate::sync::remote::bind_remote_repo;
pub use crate::sync::remote::create_remote_repo;
pub use crate::sync::remote::plan_pull_repo;
pub use crate::sync::remote::plan_push_repo;
pub use crate::sync::remote::pull_repo;
pub use crate::sync::remote::push_repo;
pub use crate::sync::remote::remote_artifact_hash;
pub use crate::sync::remote::remote_tag_references;
pub use crate::sync::remote::unbind_remote_repo;

// Artifact transfer
pub use crate::transfer::types::ArtifactDownload;
pub use crate::transfer::types::ArtifactUrlProvider;
pub use crate::transfer::types::TransferConcurrencyPolicy;
pub use crate::transfer::types::TransferStats;

// CLI hosting
pub use crate::cli::CommandLineHost;
pub use crate::cli::RemoteProviderFactory;
pub use crate::cli::run_command_line;
