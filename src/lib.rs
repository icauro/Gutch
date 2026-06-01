#![warn(clippy::all)]
#![warn(rust_2024_compatibility)]

extern crate self as gutch;

pub mod artifact;
pub mod build;
pub mod build_scan;
pub mod build_types;
pub mod config;
pub mod core;
pub mod error;
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
    BuildConfig, BuildProgressSink, BuildResult, BuildTimings, BuiltBundle, ChunkPayload,
    ExistingChunkLocation,
};
pub use core::target_identity::{
    ResolvedTargetIdentity, parse_cli_tags, resolve_target_identity,
    resolve_target_identity_from_rules,
};
pub use error::{Error, Result};
pub use install::{
    GutchBundleDownload, TargetInstallApplyResult, TargetInstallApplySource,
    TargetInstallFetchRequest, TargetInstallFetchResult, apply_target_install_with_timings,
    apply_target_install_with_timings_and_hints, fetch_target_install_content,
};
pub use plugin::{DependencyGraph, ResolvedTarget, resolve_plugin_dependencies};
pub use reconstruct::{
    ReconstructTimings, VerifiedSourceInstall,
    reconstruct_from_bundle_paths_and_verified_source_with_timings,
    reconstruct_from_bundle_paths_and_verified_source_with_timings_and_hints,
    reconstruct_from_bundle_paths_with_timings,
    reconstruct_from_bundle_paths_with_timings_and_hints, reconstruct_from_target_manifest,
    reconstruct_from_target_manifest_with_timings,
};
pub use repo::{
    GutchRepoPaths, RepoInspectStatus, RepoInspectTargetStatus, SourceTargetInstallCandidate,
    add_built_bundles_to_chunk_index, bundle_id_from_bundle_path, bundle_path, bundle_path_in_root,
    compute_target_id_from_files, discover_source_target_installs, ensure_gutch_repo_layout,
    inspect_gutch_repo, load_chunk_index, load_gutch_repo_manifest, load_target_manifest,
    serialize_target_manifest_for_remote, target_manifest_bin_path, target_manifest_path,
    write_chunk_index, write_gutch_repo_manifest, write_target_manifest_artifacts,
};
pub use runtime::{
    ChunkStorageMode, EffectiveMemoryMode, MemoryMode, RuntimePlan, RuntimeResourceHints,
    plan_build_runtime, plan_reconstruct_runtime, plan_verify_runtime,
};
pub use verify::{
    RepairClassification, RepairIssue, RepairIssueKind, VerifyTimings, classify_directory,
    classify_directory_with_progress, classify_directory_with_timings,
    classify_directory_with_timings_and_hints, estimate_build, verify_directory,
    verify_directory_with_progress, verify_directory_with_timings,
    verify_directory_with_timings_and_hints,
};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
