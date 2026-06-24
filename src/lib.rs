#![warn(clippy::all)]
#![warn(rust_2024_compatibility)]

extern crate self as gutch;

pub mod artifact;
pub mod build;
pub mod build_scan;
pub mod build_types;
pub mod cli;
pub mod config;
pub mod core;
pub mod error;
pub mod helpers;
pub mod install;
pub mod manifest;
mod path_guard;
pub mod plugin;
pub mod remote;
pub mod repo;
pub mod runtime;
#[cfg(test)]
mod test_util;
pub mod verify;

pub use artifact::{bundle, chunk, compress, hash, reconstruct};

pub use build::{
    build_from_directory, build_from_directory_with_timings,
    build_from_directory_with_timings_against_existing_bundles,
    build_from_directory_with_timings_against_existing_store,
    build_from_directory_with_timings_against_existing_store_and_manifests,
    build_from_directory_with_timings_against_existing_store_and_manifests_and_profile,
    build_from_directory_with_timings_against_existing_store_and_manifests_and_profiles,
    build_with_config,
};
pub use build_types::{
    BuildConfig, BuildProgressSink, BuildResult, BuildTimings, BuiltBundle, BuiltLooseChunk,
    ChunkPayload, ExistingChunkLocation,
};
pub use core::target_identity::{
    ResolvedTargetIdentity, parse_cli_tags, resolve_target_identity,
    resolve_target_identity_from_rules,
};
pub use error::{Error, Result};
pub use install::{
    GutchBundleDownload, TargetInstallApplyResult, TargetInstallApplySource,
    TargetInstallFetchRequest, TargetInstallFetchResult, apply_target_install_with_timings,
    apply_target_install_with_timings_and_hints, default_target_install_staging_dir,
    fetch_target_install_content,
};
pub use plugin::{DependencyGraph, ResolvedTarget, resolve_plugin_dependencies};
pub use reconstruct::{
    ReconstructOptions, ReconstructTimings, VerifiedSourceInstall,
    reconstruct_from_bundle_paths_and_verified_source_with_timings,
    reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints,
    reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints_with_options,
    reconstruct_from_bundle_paths_with_timings,
    reconstruct_from_bundle_paths_with_timings_and_hints,
    reconstruct_from_repo_paths_with_timings_and_hints,
    reconstruct_from_repo_paths_with_timings_and_hints_with_options,
    reconstruct_from_target_manifest, reconstruct_from_target_manifest_with_timings,
};
pub use repo::{
    GutchRepoPaths, RepoInspectStatus, RepoInspectTargetStatus, SourceTargetInstallCandidate,
    add_built_bundles_to_chunk_index, add_built_loose_chunks_to_chunk_index,
    bundle_id_from_bundle_path, bundle_path, bundle_path_in_root, chunk_path, chunk_path_in_root,
    compute_target_id_from_files, discover_source_target_installs, ensure_gutch_repo_layout,
    inspect_gutch_repo, load_chunk_index, load_gutch_repo_manifest, load_target_manifest,
    LookupTargetChangePlan, plan_lookup_target_changes,
    serialize_target_manifest_for_remote, target_manifest_bin_path, target_manifest_path,
    write_chunk_index, write_gutch_repo_manifest, write_target_manifest_artifacts,
};
pub use manifest::{EditableTargetTagDefinition, sanitize_editable_target_tag_schema};
pub use runtime::{
    ChunkStorageMode, EffectiveMemoryMode, MemoryMode, RuntimePlan, RuntimeResourceHints,
    plan_build_runtime, plan_reconstruct_runtime, plan_verify_runtime,
};
pub use verify::{
    RepairClassification, RepairIssue, RepairIssueKind, VerifyMode, VerifyTimings,
    classify_directory, classify_directory_with_progress,
    classify_directory_with_progress_and_mode, classify_directory_with_timings,
    classify_directory_with_timings_and_hints, classify_directory_with_timings_and_hints_and_mode,
    estimate_build, verify_directory, verify_directory_strict, verify_directory_with_progress,
    verify_directory_with_progress_and_mode, verify_directory_with_timings,
    verify_directory_with_timings_and_hints, verify_directory_with_timings_and_hints_and_mode,
};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
