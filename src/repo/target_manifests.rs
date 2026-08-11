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

pub fn target_manifest_bin_path(paths: &GutchRepoPaths, target_id: &str) -> PathBuf {
    paths
        .targets_root
        .join(target_id)
        .join(TARGET_BLUEPRINT_FILE)
}

pub fn load_target_manifest(paths: &GutchRepoPaths, target_id: &str) -> Result<TargetManifest> {
    let manifest_path = target_manifest_path(paths, target_id);
    let manifest_json = helpers::read_to_string(&manifest_path)?;
    load_target_manifest_from_json_text(target_id, &manifest_json)
}

pub fn serialize_target_manifest_for_remote(target_manifest: &TargetManifest) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(target_manifest).map_err(Into::into)
}

pub fn write_target_manifest_artifacts(
    paths: &GutchRepoPaths,
    target_manifest: &TargetManifest,
) -> Result<(PathBuf, u64)> {
    helpers::create_dir_all(&paths.targets_root)?;
    helpers::create_dir_all(&paths.targets_root.join(&target_manifest.target_id))?;

    let manifest_path = target_manifest_path(paths, &target_manifest.target_id);
    let bin_path = target_manifest_bin_path(paths, &target_manifest.target_id);

    // target_metadata.json is the canonical metadata representation for both
    // local repositories and synchronized/remote targets. The binary file is
    // retained only as an internal transfer/cache optimization for the file
    // blueprint.
    let manifest_json = serde_json::to_vec_pretty(target_manifest)?;
    helpers::write(&manifest_path, &manifest_json)?;
    binary::write_target_manifest_files_binary(&bin_path, target_manifest)?;

    let manifest_size = std::fs::metadata(&manifest_path)?.len();
    Ok((manifest_path, manifest_size))
}

pub(crate) fn load_target_manifest_from_json_text(
    target_id: &str,
    manifest_json: &str,
) -> Result<TargetManifest> {
    let mut manifest: TargetManifest = serde_json::from_str(manifest_json)?;
    if manifest.target_id.trim().is_empty() {
        manifest.target_id = target_id.to_string();
    }
    Ok(manifest)
}
