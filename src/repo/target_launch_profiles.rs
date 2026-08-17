use std::path::Path;

use crate::manifest::{
    TargetDirectRuntimeMetadata, TargetLaunchProfile, TargetLaunchProfilesDocument,
    TargetProductMetadata, TargetRuntimeMetadata,
};
use crate::{Error, Result};

use super::inventory::list_repo_target_ids;
use super::{GutchRepoPaths, load_target_manifest};

pub fn load_target_launch_profiles(paths: &GutchRepoPaths) -> Result<TargetLaunchProfilesDocument> {
    let document = load_target_launch_profiles_document(paths)?;
    document.validate_repository_targets(&list_repo_target_ids(paths)?)?;
    Ok(document)
}

fn load_target_launch_profiles_document(
    paths: &GutchRepoPaths,
) -> Result<TargetLaunchProfilesDocument> {
    if !paths.target_launch_profiles_path.is_file() {
        return Err(Error::manifest(format!(
            "target launch profiles not found at {}",
            paths.target_launch_profiles_path.display()
        )));
    }
    let document: TargetLaunchProfilesDocument = crate::helpers::from_json_str(
        &crate::helpers::read_to_string(&paths.target_launch_profiles_path)?,
    )?;
    document.validate()?;
    Ok(document)
}

pub fn write_target_launch_profiles(
    paths: &GutchRepoPaths,
    document: &TargetLaunchProfilesDocument,
) -> Result<()> {
    document.validate_repository_targets(&list_repo_target_ids(paths)?)?;
    crate::helpers::write_json(&paths.target_launch_profiles_path, document)
}

pub fn create_target_launch_profile(
    repo_root: &Path,
    profile_id: &str,
    mut profile: TargetLaunchProfile,
) -> Result<()> {
    crate::manifest::target_launch_profiles::validate_profile_id(profile_id)?;
    profile.assigned_target_ids.clear();
    let (paths, _) = super::load_gutch_repo_manifest(repo_root)?;
    let mut document = load_target_launch_profiles(&paths)?;
    if document
        .target_launch_profiles
        .insert(profile_id.to_string(), profile)
        .is_some()
    {
        return Err(Error::manifest(format!(
            "target launch profile '{profile_id}' already exists"
        )));
    }
    write_target_launch_profiles(&paths, &document)
}

pub fn update_target_launch_profile(
    repo_root: &Path,
    profile_id: &str,
    mut profile: TargetLaunchProfile,
) -> Result<()> {
    let (paths, _) = super::load_gutch_repo_manifest(repo_root)?;
    let mut document = load_target_launch_profiles(&paths)?;
    let existing = document
        .target_launch_profiles
        .get(profile_id)
        .ok_or_else(|| {
            Error::manifest(format!("target launch profile '{profile_id}' not found"))
        })?;
    profile.assigned_target_ids = existing.assigned_target_ids.clone();
    document
        .target_launch_profiles
        .insert(profile_id.to_string(), profile);
    write_target_launch_profiles(&paths, &document)
}

pub fn set_default_target_launch_profile(repo_root: &Path, profile_id: &str) -> Result<()> {
    let (paths, _) = super::load_gutch_repo_manifest(repo_root)?;
    let mut document = load_target_launch_profiles(&paths)?;
    if !document.target_launch_profiles.contains_key(profile_id) {
        return Err(Error::manifest(format!(
            "target launch profile '{profile_id}' not found"
        )));
    }
    document.default_target_launch_profile_id = profile_id.to_string();
    write_target_launch_profiles(&paths, &document)
}

pub fn delete_target_launch_profile(repo_root: &Path, profile_id: &str) -> Result<()> {
    let (paths, _) = super::load_gutch_repo_manifest(repo_root)?;
    let mut document = load_target_launch_profiles(&paths)?;
    if document.default_target_launch_profile_id == profile_id {
        return Err(Error::manifest(
            "default target launch profile cannot be deleted",
        ));
    }
    let profile = document
        .target_launch_profiles
        .get(profile_id)
        .ok_or_else(|| {
            Error::manifest(format!("target launch profile '{profile_id}' not found"))
        })?;
    if !profile.assigned_target_ids.is_empty() {
        return Err(Error::manifest(
            "assigned target launch profile cannot be deleted",
        ));
    }
    document.target_launch_profiles.remove(profile_id);
    write_target_launch_profiles(&paths, &document)
}

pub fn assign_target_launch_profile(
    repo_root: &Path,
    target_id: &str,
    profile_id: &str,
) -> Result<()> {
    let (paths, _) = super::load_gutch_repo_manifest(repo_root)?;
    if !super::target_manifest_path(&paths, target_id).is_file() {
        return Err(Error::manifest(format!("target '{target_id}' not found")));
    }
    let mut document = load_target_launch_profiles(&paths)?;
    if !document.target_launch_profiles.contains_key(profile_id) {
        return Err(Error::manifest(format!(
            "target launch profile '{profile_id}' not found"
        )));
    }
    for profile in document.target_launch_profiles.values_mut() {
        profile
            .assigned_target_ids
            .retain(|assigned_target_id| assigned_target_id != target_id);
    }
    document
        .target_launch_profiles
        .get_mut(profile_id)
        .expect("profile existence checked")
        .assigned_target_ids
        .push(target_id.to_string());
    write_target_launch_profiles(&paths, &document)
}

pub(crate) fn assign_new_target_to_default(paths: &GutchRepoPaths, target_id: &str) -> Result<()> {
    let mut document = load_target_launch_profiles_document(paths)?;
    if document.assigned_profile_id(target_id).is_some() {
        return Ok(());
    }
    document
        .target_launch_profiles
        .get_mut(&document.default_target_launch_profile_id)
        .expect("validated default profile")
        .assigned_target_ids
        .push(target_id.to_string());
    write_target_launch_profiles(paths, &document)
}

pub(crate) fn remove_target_assignment(paths: &GutchRepoPaths, target_id: &str) -> Result<()> {
    let mut document = load_target_launch_profiles_document(paths)?;
    for profile in document.target_launch_profiles.values_mut() {
        profile
            .assigned_target_ids
            .retain(|assigned_target_id| assigned_target_id != target_id);
    }
    write_target_launch_profiles(paths, &document)
}

pub fn materialize_target_runtime(
    paths: &GutchRepoPaths,
    target_id: &str,
) -> Result<crate::manifest::target::TargetManifest> {
    let document = load_target_launch_profiles(paths)?;
    let profile_id = document.assigned_profile_id(target_id).ok_or_else(|| {
        Error::manifest(format!(
            "target '{target_id}' is not assigned to a target launch profile"
        ))
    })?;
    let profile = &document.target_launch_profiles[profile_id];
    if profile.launch_actions.is_empty() {
        return Err(Error::manifest(format!(
            "target launch profile '{profile_id}' has no launch actions"
        )));
    }
    let mut target = load_target_manifest(paths, target_id)?;
    let launch = crate::manifest::launch::ResolvedLaunchConfiguration {
        primary: profile
            .launch_actions
            .first()
            .map(|action| action.action_name.clone()),
        actions: profile
            .launch_actions
            .iter()
            .map(|action| crate::manifest::launch::ResolvedLaunchAction {
                name: action.action_name.clone(),
                executable: action.executable_path.clone(),
                arguments: action.command_arguments.clone(),
                working_directory: action.working_directory_path.clone(),
            })
            .collect(),
    };
    crate::manifest::launch::validate_resolved_launch_for_target(&launch, &target)?;
    target.launch = Some(launch);
    target.product_metadata = Some(TargetProductMetadata {
        document_version: crate::manifest::FORMAT_VERSION.to_string(),
        runtime: TargetRuntimeMetadata::Direct(TargetDirectRuntimeMetadata {
            launch_actions: profile.launch_actions.clone(),
            external_dependencies: profile.external_dependencies.clone(),
        }),
    });
    Ok(target)
}
