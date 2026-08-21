use std::path::PathBuf;

use crate::manifest::{
    binary,
    external_dependency::ExternalProductDependencyRequirement,
    launch::ResolvedLaunchConfiguration,
    target::{TargetManifest, TargetProductMetadata},
};
use crate::{Result, helpers};

use super::GutchRepoPaths;

pub const TARGET_METADATA_FILE: &str = "target_metadata.json";
pub const TARGET_PRODUCT_METADATA_FILE: &str = "target_product_metadata.json";
pub const TARGET_BLUEPRINT_FILE: &str = "target_blueprint.gtcbin";

pub fn validate_target_id(target_id: &str) -> Result<()> {
    if target_id.len() == 64
        && target_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(crate::Error::manifest(format!(
            "invalid BLAKE3 target id '{target_id}'"
        )))
    }
}

pub fn target_manifest_path(paths: &GutchRepoPaths, target_id: &str) -> PathBuf {
    paths
        .targets_root
        .join(target_id)
        .join(TARGET_METADATA_FILE)
}

pub fn target_blueprint_path(paths: &GutchRepoPaths, target_id: &str) -> PathBuf {
    paths
        .targets_root
        .join(target_id)
        .join(TARGET_BLUEPRINT_FILE)
}

pub fn target_product_metadata_path(paths: &GutchRepoPaths, target_id: &str) -> PathBuf {
    paths
        .targets_root
        .join(target_id)
        .join(TARGET_PRODUCT_METADATA_FILE)
}

pub fn load_target_manifest(paths: &GutchRepoPaths, target_id: &str) -> Result<TargetManifest> {
    let manifest_path = target_manifest_path(paths, target_id);
    let manifest_json = helpers::read_to_string(&manifest_path)?;
    let mut manifest = load_target_manifest_from_json_text(target_id, &manifest_json)?;
    manifest.product_metadata = load_target_product_metadata(paths, target_id)?;
    let blueprint_path = target_blueprint_path(paths, target_id);
    if !blueprint_path.is_file() {
        return Err(crate::Error::manifest(format!(
            "target blueprint not found at {}",
            blueprint_path.display()
        )));
    }
    manifest.files = binary::read_target_blueprint_binary(&blueprint_path)?;
    manifest.total_logical_size = manifest.files.iter().map(|file| file.size).sum();
    manifest.sync_launch_from_product_metadata();
    if manifest.product_metadata.is_some() && manifest.launch.is_none() {
        return Err(crate::Error::manifest(
            "target product runtime has no matching host variant",
        ));
    }
    if let Some(launch) = manifest.launch.as_ref() {
        crate::manifest::launch::validate_resolved_launch_for_target(launch, &manifest)?;
    }
    Ok(manifest)
}

pub fn serialize_target_manifest_for_remote(target_manifest: &TargetManifest) -> Result<Vec<u8>> {
    let mut metadata = target_manifest.clone();
    metadata.product_metadata = None;
    metadata.launch = None;
    metadata.files.clear();
    serde_json::to_vec_pretty(&metadata).map_err(Into::into)
}

pub fn serialize_target_product_metadata_for_remote(
    target_manifest: &TargetManifest,
) -> Result<Option<Vec<u8>>> {
    let mut metadata = target_manifest.clone();
    metadata.sync_product_metadata_from_launch();
    let Some(product_metadata) = metadata.product_metadata.as_ref() else {
        return Ok(None);
    };
    product_metadata.validate_structure()?;
    serde_json::to_vec_pretty(product_metadata)
        .map(Some)
        .map_err(Into::into)
}

pub fn load_target_product_metadata(
    paths: &GutchRepoPaths,
    target_id: &str,
) -> Result<Option<TargetProductMetadata>> {
    let metadata_path = target_product_metadata_path(paths, target_id);
    if !metadata_path.is_file() {
        return Ok(None);
    }
    let metadata: TargetProductMetadata =
        helpers::from_json_str(&helpers::read_to_string(&metadata_path)?)?;
    metadata.validate_structure()?;
    Ok(Some(metadata))
}

pub fn write_target_product_metadata(
    paths: &GutchRepoPaths,
    target_id: &str,
    metadata: Option<&TargetProductMetadata>,
) -> Result<Option<(PathBuf, u64)>> {
    let metadata_path = target_product_metadata_path(paths, target_id);
    if let Some(metadata) = metadata {
        metadata.validate_structure()?;
        helpers::create_dir_all(&paths.targets_root.join(target_id))?;
        let bytes = serde_json::to_vec_pretty(metadata)?;
        helpers::write(&metadata_path, &bytes)?;
        let size = std::fs::metadata(&metadata_path)?.len();
        return Ok(Some((metadata_path, size)));
    }
    if metadata_path.exists() {
        helpers::remove_file(&metadata_path)?;
    }
    Ok(None)
}

pub fn write_target_product_metadata_only(
    paths: &GutchRepoPaths,
    target_manifest: &TargetManifest,
) -> Result<()> {
    let mut metadata = target_manifest.clone();
    metadata.sync_product_metadata_from_launch();
    write_target_product_metadata(
        paths,
        &target_manifest.target_id,
        metadata.product_metadata.as_ref(),
    )?;
    Ok(())
}

pub fn save_target_product_metadata(
    repo_root: &std::path::Path,
    target_id: &str,
    launch: Option<ResolvedLaunchConfiguration>,
    dependencies: Vec<ExternalProductDependencyRequirement>,
) -> Result<()> {
    let paths = GutchRepoPaths::from_repo_root(repo_root);
    let mut target_manifest = load_target_manifest(&paths, target_id)?;
    target_manifest.launch = launch;
    target_manifest.product_metadata = target_manifest
        .launch
        .as_ref()
        .map(|launch| TargetProductMetadata::from_launch(launch, dependencies));
    if let Some(launch) = target_manifest.launch.as_ref() {
        crate::manifest::launch::validate_resolved_launch_for_target(launch, &target_manifest)?;
    }
    write_target_product_metadata(&paths, target_id, target_manifest.product_metadata.as_ref())?;
    Ok(())
}

pub fn write_target_manifest_artifacts(
    paths: &GutchRepoPaths,
    target_manifest: &TargetManifest,
) -> Result<(PathBuf, u64)> {
    helpers::create_dir_all(&paths.targets_root)?;
    helpers::create_dir_all(&paths.targets_root.join(&target_manifest.target_id))?;

    let manifest_path = target_manifest_path(paths, &target_manifest.target_id);
    let bin_path = target_blueprint_path(paths, &target_manifest.target_id);

    // target_metadata.json is the canonical target index. The sibling
    // target_product_metadata.json owns product runtime data, while
    // target_blueprint.gtcbin owns file and chunk records.
    let mut metadata = target_manifest.clone();
    metadata.sync_product_metadata_from_launch();
    let product_metadata = metadata.product_metadata.clone();
    metadata.files.clear();
    metadata.launch = None;
    metadata.product_metadata = None;
    let manifest_json = serde_json::to_vec_pretty(&metadata)?;
    helpers::write(&manifest_path, &manifest_json)?;
    write_target_product_metadata(paths, &target_manifest.target_id, product_metadata.as_ref())?;
    binary::write_target_blueprint_binary(&bin_path, target_manifest)?;

    let manifest_size = std::fs::metadata(&manifest_path)?.len();
    Ok((manifest_path, manifest_size))
}

pub(crate) fn load_target_manifest_from_json_text(
    target_id: &str,
    manifest_json: &str,
) -> Result<TargetManifest> {
    let mut manifest: TargetManifest = serde_json::from_str(manifest_json)?;
    manifest.target_id = target_id.to_string();
    Ok(manifest)
}

pub(crate) fn write_target_metadata_only(
    paths: &GutchRepoPaths,
    target_manifest: &TargetManifest,
) -> Result<()> {
    helpers::create_dir_all(&paths.targets_root.join(&target_manifest.target_id))?;
    let mut metadata = target_manifest.clone();
    metadata.launch = None;
    metadata.product_metadata = None;
    metadata.files.clear();
    helpers::write(
        &target_manifest_path(paths, &target_manifest.target_id),
        &serde_json::to_vec_pretty(&metadata)?,
    )?;
    Ok(())
}
