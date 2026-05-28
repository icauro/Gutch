#![warn(clippy::all)]
#![warn(rust_2024_compatibility)]

extern crate self as gutch;

pub mod build;
pub mod build_scan;
pub mod build_types;
pub mod bundle;
pub mod chunk;
#[path = "cli/identity.rs"]
pub mod cli_identity;
pub mod compress;
pub mod error;
pub mod hash;
pub mod install;
pub mod manifest;
mod path_guard;
pub mod plugin;
pub mod reconstruct;
pub mod repo;
pub mod runtime;
#[cfg(test)]
mod test_util;
pub mod verify;

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
    BuildConfig, BuildResult, BuildTimings, BuiltBundle, ChunkPayload, ExistingChunkLocation,
};
pub use cli_identity::{
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
    reconstruct_from_bundle_paths_with_timings, reconstruct_from_target_manifest,
    reconstruct_from_bundle_paths_with_timings_and_hints,
    reconstruct_from_target_manifest_with_timings,
};
pub use repo::{
    ContentRepoPaths, RepoInspectStatus, RepoInspectTargetStatus, SourceTargetInstallCandidate,
    discover_source_target_installs, ensure_content_repo_layout, generate_target_id,
    inspect_content_repo, load_chunk_index, load_content_repo_manifest, target_manifest_path,
    write_chunk_index, write_content_repo_manifest,
};
pub use runtime::{
    ChunkStorageMode, EffectiveMemoryMode, MemoryMode, RuntimePlan, RuntimeResourceHints,
    plan_build_runtime, plan_reconstruct_runtime, plan_verify_runtime,
};
pub use verify::{
    RepairClassification, RepairIssue, RepairIssueKind, VerifyTimings, classify_directory,
    classify_directory_with_timings, classify_directory_with_timings_and_hints, estimate_build,
    verify_directory, verify_directory_with_timings, verify_directory_with_timings_and_hints,
};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
