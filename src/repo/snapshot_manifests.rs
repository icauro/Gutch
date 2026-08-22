use std::path::PathBuf;

use crate::manifest::{
    binary,
    external_dependency::ExternalProductDependencyRequirement,
    launch::ResolvedLaunchConfiguration,
    snapshot::{ProductMetadata, SnapshotManifest},
};
use crate::{Result, helpers};

use super::GutchRepoPaths;

pub const SNAPSHOT_INDEX_FILE: &str = "snapshot_index.json";
pub const PRODUCT_METADATA_FILE: &str = "product_metadata.json";
pub const SNAPSHOT_BLUEPRINT_FILE: &str = "snapshot_blueprint.gtcbin";

pub fn validate_snapshot_id(snapshot_id: &str) -> Result<()> {
    if snapshot_id.len() == 64
        && snapshot_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(crate::Error::manifest(format!(
            "invalid BLAKE3 snapshot id '{snapshot_id}'"
        )))
    }
}

pub fn snapshot_manifest_path(paths: &GutchRepoPaths, snapshot_id: &str) -> PathBuf {
    paths
        .snapshots_root
        .join(snapshot_id)
        .join(SNAPSHOT_INDEX_FILE)
}

pub fn snapshot_blueprint_path(paths: &GutchRepoPaths, snapshot_id: &str) -> PathBuf {
    paths
        .snapshots_root
        .join(snapshot_id)
        .join(SNAPSHOT_BLUEPRINT_FILE)
}

pub fn product_metadata_path(paths: &GutchRepoPaths, snapshot_id: &str) -> PathBuf {
    paths
        .snapshots_root
        .join(snapshot_id)
        .join(PRODUCT_METADATA_FILE)
}

pub fn load_snapshot_manifest(
    paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<SnapshotManifest> {
    let manifest_path = snapshot_manifest_path(paths, snapshot_id);
    let manifest_json = helpers::read_to_string(&manifest_path)?;
    let mut manifest = load_snapshot_manifest_from_json_text(snapshot_id, &manifest_json)?;
    manifest.product_metadata = load_product_metadata(paths, snapshot_id)?;
    let blueprint_path = snapshot_blueprint_path(paths, snapshot_id);
    if !blueprint_path.is_file() {
        return Err(crate::Error::manifest(format!(
            "snapshot blueprint not found at {}",
            blueprint_path.display()
        )));
    }
    manifest.files = binary::read_snapshot_blueprint_binary(&blueprint_path)?;
    manifest.total_logical_size = manifest.files.iter().map(|file| file.size).sum();
    manifest.sync_launch_from_product_metadata();
    if manifest.product_metadata.is_some() && manifest.launch.is_none() {
        return Err(crate::Error::manifest(
            "snapshot product runtime has no matching host variant",
        ));
    }
    if let Some(launch) = manifest.launch.as_ref() {
        crate::manifest::launch::validate_resolved_launch_for_snapshot(launch, &manifest)?;
    }
    Ok(manifest)
}

pub fn serialize_snapshot_manifest_for_remote(
    snapshot_manifest: &SnapshotManifest,
) -> Result<Vec<u8>> {
    let mut metadata = snapshot_manifest.clone();
    metadata.product_metadata = None;
    metadata.launch = None;
    metadata.files.clear();
    serde_json::to_vec_pretty(&metadata).map_err(Into::into)
}

pub fn serialize_product_metadata_for_remote(
    snapshot_manifest: &SnapshotManifest,
) -> Result<Option<Vec<u8>>> {
    let mut metadata = snapshot_manifest.clone();
    metadata.sync_product_metadata_from_launch();
    let Some(product_metadata) = metadata.product_metadata.as_ref() else {
        return Ok(None);
    };
    product_metadata.validate_structure()?;
    serde_json::to_vec_pretty(product_metadata)
        .map(Some)
        .map_err(Into::into)
}

pub fn load_product_metadata(
    paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<Option<ProductMetadata>> {
    let metadata_path = product_metadata_path(paths, snapshot_id);
    if !metadata_path.is_file() {
        return Ok(None);
    }
    let metadata: ProductMetadata =
        helpers::from_json_str(&helpers::read_to_string(&metadata_path)?)?;
    metadata.validate_structure()?;
    Ok(Some(metadata))
}

pub fn write_product_metadata(
    paths: &GutchRepoPaths,
    snapshot_id: &str,
    metadata: Option<&ProductMetadata>,
) -> Result<Option<(PathBuf, u64)>> {
    let metadata_path = product_metadata_path(paths, snapshot_id);
    if let Some(metadata) = metadata {
        metadata.validate_structure()?;
        helpers::create_dir_all(&paths.snapshots_root.join(snapshot_id))?;
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

pub fn write_product_metadata_only(
    paths: &GutchRepoPaths,
    snapshot_manifest: &SnapshotManifest,
) -> Result<()> {
    let mut metadata = snapshot_manifest.clone();
    metadata.sync_product_metadata_from_launch();
    write_product_metadata(
        paths,
        &snapshot_manifest.snapshot_id,
        metadata.product_metadata.as_ref(),
    )?;
    Ok(())
}

pub fn save_product_metadata(
    repo_root: &std::path::Path,
    snapshot_id: &str,
    launch: Option<ResolvedLaunchConfiguration>,
    dependencies: Vec<ExternalProductDependencyRequirement>,
) -> Result<()> {
    let paths = GutchRepoPaths::from_repo_root(repo_root);
    let mut snapshot_manifest = load_snapshot_manifest(&paths, snapshot_id)?;
    snapshot_manifest.launch = launch;
    snapshot_manifest.product_metadata = snapshot_manifest
        .launch
        .as_ref()
        .map(|launch| ProductMetadata::from_launch(launch, dependencies));
    if let Some(launch) = snapshot_manifest.launch.as_ref() {
        crate::manifest::launch::validate_resolved_launch_for_snapshot(launch, &snapshot_manifest)?;
    }
    write_product_metadata(
        &paths,
        snapshot_id,
        snapshot_manifest.product_metadata.as_ref(),
    )?;
    Ok(())
}

pub fn write_snapshot_manifest_artifacts(
    paths: &GutchRepoPaths,
    snapshot_manifest: &SnapshotManifest,
) -> Result<(PathBuf, u64)> {
    helpers::create_dir_all(&paths.snapshots_root)?;
    helpers::create_dir_all(&paths.snapshots_root.join(&snapshot_manifest.snapshot_id))?;

    let manifest_path = snapshot_manifest_path(paths, &snapshot_manifest.snapshot_id);
    let bin_path = snapshot_blueprint_path(paths, &snapshot_manifest.snapshot_id);

    // snapshot_index.json is the canonical snapshot index. The sibling
    // product_metadata.json owns product runtime data, while
    // snapshot_blueprint.gtcbin owns file and chunk records.
    let mut metadata = snapshot_manifest.clone();
    metadata.sync_product_metadata_from_launch();
    let product_metadata = metadata.product_metadata.clone();
    metadata.files.clear();
    metadata.launch = None;
    metadata.product_metadata = None;
    let manifest_json = serde_json::to_vec_pretty(&metadata)?;
    helpers::write(&manifest_path, &manifest_json)?;
    write_product_metadata(
        paths,
        &snapshot_manifest.snapshot_id,
        product_metadata.as_ref(),
    )?;
    binary::write_snapshot_blueprint_binary(&bin_path, snapshot_manifest)?;

    let manifest_size = std::fs::metadata(&manifest_path)?.len();
    Ok((manifest_path, manifest_size))
}

pub(crate) fn load_snapshot_manifest_from_json_text(
    snapshot_id: &str,
    manifest_json: &str,
) -> Result<SnapshotManifest> {
    let mut manifest: SnapshotManifest = serde_json::from_str(manifest_json)?;
    manifest.snapshot_id = snapshot_id.to_string();
    Ok(manifest)
}

pub(crate) fn write_snapshot_index_only(
    paths: &GutchRepoPaths,
    snapshot_manifest: &SnapshotManifest,
) -> Result<()> {
    helpers::create_dir_all(&paths.snapshots_root.join(&snapshot_manifest.snapshot_id))?;
    let mut metadata = snapshot_manifest.clone();
    metadata.launch = None;
    metadata.product_metadata = None;
    metadata.files.clear();
    helpers::write(
        &snapshot_manifest_path(paths, &snapshot_manifest.snapshot_id),
        &serde_json::to_vec_pretty(&metadata)?,
    )?;
    Ok(())
}
