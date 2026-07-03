use std::path::Path;

use crate::Result;
use crate::manifest;

use super::{
    GutchRepoPaths, load_gutch_repo_manifest, load_target_manifest, write_gutch_repo_manifest,
    write_target_manifest_artifacts,
};

pub fn initialize_tracked_gutch_repo(
    repo_root: &Path,
    target_schema: Option<manifest::TargetTagSchema>,
    target_sources_path: Option<String>,
) -> Result<()> {
    super::init_policy::initialize_tracked_gutch_repo_with_options(
        repo_root,
        target_schema,
        target_sources_path,
        super::init_policy::RepoInitOptions { force: false },
    )
}

pub fn update_gutch_repo_schema(
    repo_root: &Path,
    target_schema: Option<manifest::TargetTagSchema>,
) -> Result<()> {
    let (repo_paths, mut repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    repo_manifest.target_schema = target_schema;
    write_gutch_repo_manifest(&repo_paths, &repo_manifest)
}

pub fn update_gutch_repo_launch(
    repo_root: &Path,
    launch: Option<manifest::launch::RepoLaunchConfiguration>,
) -> Result<()> {
    if let Some(configuration) = launch.as_ref() {
        manifest::launch::validate_repo_launch_configuration(configuration)?;
    }
    let (repo_paths, mut repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    for target_id in repo_target_ids(&repo_paths)? {
        let target = load_target_manifest(&repo_paths, &target_id)?;
        if let Some(override_configuration) = target.launch.as_ref() {
            manifest::launch::validate_resolved_launch_for_target(override_configuration, &target)?;
        } else if let Some(configuration) = launch.as_ref() {
            manifest::launch::resolve_repo_launch_for_target(configuration, &target)?;
        }
    }
    repo_manifest.launch = launch;
    write_gutch_repo_manifest(&repo_paths, &repo_manifest)
}

pub fn update_gutch_target_launch(
    repo_root: &Path,
    target_id: &str,
    launch: Option<manifest::launch::ResolvedLaunchConfiguration>,
) -> Result<()> {
    let (repo_paths, repo_manifest) = load_gutch_repo_manifest(repo_root)?;
    let mut target = load_target_manifest(&repo_paths, target_id)?;
    if let Some(configuration) = launch.as_ref() {
        manifest::launch::validate_resolved_launch_for_target(configuration, &target)?;
    } else if let Some(configuration) = repo_manifest.launch.as_ref() {
        manifest::launch::resolve_repo_launch_for_target(configuration, &target)?;
    }
    target.launch = launch;
    write_target_manifest_artifacts(&repo_paths, &target)?;
    Ok(())
}

fn repo_target_ids(paths: &GutchRepoPaths) -> Result<Vec<String>> {
    if !paths.targets_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut target_ids = std::fs::read_dir(&paths.targets_root)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.path().is_file()
                && entry.path().extension().and_then(|value| value.to_str()) == Some("json")
        })
        .filter_map(|entry| {
            entry
                .path()
                .file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    target_ids.sort();
    Ok(target_ids)
}
