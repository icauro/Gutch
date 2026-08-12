use std::path::PathBuf;

use crate::manifest::{binary, target::TargetManifest};
use crate::{Result, helpers};

use super::GutchRepoPaths;

pub const TARGET_METADATA_FILE: &str = "target_metadata.json";
pub const TARGET_BLUEPRINT_FILE: &str = "target_blueprint.bin";

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

pub fn load_target_manifest(paths: &GutchRepoPaths, target_id: &str) -> Result<TargetManifest> {
    let manifest_path = target_manifest_path(paths, target_id);
    let manifest_json = helpers::read_to_string(&manifest_path)?;
    let mut manifest = load_target_manifest_from_json_text(target_id, &manifest_json)?;
    if let Some(product_metadata) = manifest.product_metadata.as_ref() {
        product_metadata.validate_structure()?;
    }
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
    metadata.sync_product_metadata_from_launch();
    if let Some(product_metadata) = metadata.product_metadata.as_ref() {
        product_metadata.validate_structure()?;
    }
    serde_json::to_vec_pretty(&metadata).map_err(Into::into)
}

pub fn write_target_manifest_artifacts(
    paths: &GutchRepoPaths,
    target_manifest: &TargetManifest,
) -> Result<(PathBuf, u64)> {
    helpers::create_dir_all(&paths.targets_root)?;
    helpers::create_dir_all(&paths.targets_root.join(&target_manifest.target_id))?;

    let manifest_path = target_manifest_path(paths, &target_manifest.target_id);
    let bin_path = target_blueprint_path(paths, &target_manifest.target_id);

    // target_metadata.json is the canonical metadata index. The sibling
    // target_blueprint.bin owns the file and chunk records.
    let mut metadata = target_manifest.clone();
    metadata.sync_product_metadata_from_launch();
    if let Some(product_metadata) = metadata.product_metadata.as_ref() {
        product_metadata.validate_structure()?;
    }
    metadata.files.clear();
    let manifest_json = serde_json::to_vec_pretty(&metadata)?;
    helpers::write(&manifest_path, &manifest_json)?;
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
    metadata.sync_product_metadata_from_launch();
    if let Some(product_metadata) = metadata.product_metadata.as_ref() {
        product_metadata.validate_structure()?;
    }
    metadata.files.clear();
    helpers::write(
        &target_manifest_path(paths, &target_manifest.target_id),
        &serde_json::to_vec_pretty(&metadata)?,
    )
}
