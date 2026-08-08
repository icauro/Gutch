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
mod install;
pub(crate) use install::target_manifest_from_download;
pub mod manifest;
mod progress;
pub mod reconstruct;
pub mod remote;
pub mod repo;
pub mod runtime;
pub mod target;
#[cfg(test)]
mod test_util;
pub mod verify;

pub use artifact::hash::ChunkHash;
pub use artifact::{bundle, chunk, compress, hash};
pub use build::scan as build_scan;
pub use build::types as build_types;
pub use target::path_guard;
pub use target::source_config;

pub use build::types::{
    BuildConfig, BuildProgressSink, BuildResult, BuildTimings, BuiltBundle, BuiltLooseChunk,
    ChunkPayload, ExistingChunkLocation,
};
pub use build::{build_from_directory, build_with_config};
pub use error::{Error, Result};
pub use install::{
    GutchBundleDownload, ResolvedTargetInstallState, TargetInstallApplyFromRepoRequest,
    TargetInstallApplyFromRepoResult, TargetInstallApplyResult, TargetInstallApplySource,
    TargetInstallDownloadAndApplyRequest, TargetInstallDownloadAndApplyResult,
    TargetInstallFetchRequest, TargetInstallFetchResult, TargetInstallRepairFromRepoRequest,
    TargetInstallRepairFromRepoResult, TargetInstallRepairPlanFromRepoRequest,
    TargetInstallRepairPlanFromRepoResult, TargetInstallRepairResult,
    TargetInstallVerifyFromRepoRequest, TargetInstallVerifyFromRepoResult,
    apply_target_install_from_repo, default_target_install_staging_dir,
    download_and_apply_target_install, fetch_target_install_content,
    fetch_target_install_content_for_required_bundle_ids, plan_repair_target_install_from_repo,
    repair_target_install_from_repo, resolve_target_install_state, verify_target_install_from_repo,
};
pub use manifest::launch::{
    LaunchPlatform, RepoLaunchAction, RepoLaunchCommand, RepoLaunchConfiguration,
    ResolvedLaunchAction, ResolvedLaunchConfiguration, resolve_repo_launch_for_target,
    validate_launch_path, validate_repo_launch_configuration, validate_resolved_launch_for_target,
};
pub use manifest::template::{
    TaggingRuleTemplate, list_templates, template_by_name, template_schema,
};
pub use manifest::{
    EditableTargetTagDefinition, ExternalProductDependencyRequirement, InferenceHandler,
    TargetTagSchemaValidationError, TargetTagSchemaValidationResult,
    sanitize_editable_target_tag_schema, validate_editable_target_tag_schema,
};
pub use progress::{
    GutchCancellationHook, GutchOperation, GutchOperationControl, GutchProgressEvent,
    GutchProgressPhase, GutchProgressSink,
};
pub use reconstruct::{
    ReconstructOptions, ReconstructTimings, VerifiedSourceInstall, reconstruct_from_bundle_paths,
    reconstruct_from_repo, reconstruct_from_target_manifest,
};
pub use repo::{
    GutchRepoPaths, PullConflictPolicy, REPO_INIT_NEEDS_CONFIRMATION, RemoteRepoEntry,
    RepoCheckStatus, RepoCheckTargetStatus, RepoCheckTimings, RepoInitAssessment, RepoInitOptions,
    RepoInitPathKind, RepoInitRisk, RepoPullPlan, RepoPullStats, RepoSyncPlan,
    RepoSyncTargetChange, RepoSyncUpStats, RepoTargetInsertRequest, RepoTargetInsertResult,
    RepoVerifyResult, RepoVerifyTimings, SourceTargetInstallCandidate,
    SourceTargetInstallDisposition, TargetInsertKind, TargetInsertPlan, TargetSourcesRefresh,
    TargetSourcesSyncOptions, TargetSourcesSyncPlan, TargetSourcesSyncResult, TargetVerifyResult,
    TrackedRepoCheckStatus, TrackedRepoCheckTimings, add_built_bundles_to_chunk_index,
    add_built_loose_chunks_to_chunk_index, assess_repo_init_path, bind_gutch_repo_remote,
    bundle_id_from_bundle_path, bundle_path, bundle_path_in_root, check_gutch_repo,
    check_tracked_gutch_repo, chunk_path, chunk_path_in_root, cleanup_orphan_bundles_in_repo,
    compute_target_id_from_files, delete_repo_target_and_cleanup, delete_repo_targets,
    delete_repo_targets_with_matching_tags, describe_target_insert_plan,
    describe_target_sources_sync_plan, discover_source_target_installs, ensure_gutch_repo_layout,
    ensure_gutch_repo_tracked, initialize_tracked_gutch_repo,
    initialize_tracked_gutch_repo_with_options, insert_repo_target, link_repo, list_remote_repos,
    load_chunk_index, load_gutch_repo_manifest, load_target_manifest,
    parse_repo_init_needs_confirmation_error, plan_repo_pull, plan_sync_up_repo,
    plan_target_insert, plan_target_sources_sync, preview_source_target_install, pull_repo,
    remove_target_from_repo, repo_init_needs_confirmation_error,
    serialize_target_manifest_for_remote, sync_target_sources, sync_up_repo,
    target_manifest_bin_path, target_manifest_path, unlink_repo,
    update_gutch_repo_external_product_dependencies, update_gutch_repo_launch,
    update_gutch_repo_name, update_gutch_repo_schema, update_gutch_target_launch,
    verify_gutch_repo, write_chunk_index, write_gutch_repo_manifest,
    write_target_manifest_artifacts,
};
pub use runtime::{
    ChunkStorageMode, EffectiveMemoryMode, MemoryMode, RuntimePlan, RuntimeResourceHints,
    plan_build_runtime, plan_reconstruct_runtime, plan_verify_runtime,
};
pub use target::identity::{
    ResolvedTargetIdentity, parse_cli_tags, resolve_target_identity,
    resolve_target_identity_from_rules, resolve_target_identity_with_inference,
};
pub use target::path_guard::{
    ExistingInstallPathKind, resolve_existing_install_path, validate_manifest_relative_path,
};
pub use verify::{
    RepairClassification, RepairIssue, RepairIssueKind, VerifyMode, VerifyRequest, VerifyTimings,
    classify_directory, estimate_build, verify_directory,
};

#[cfg(test)]
mod tests;
