use std::path::PathBuf;

use crate::Result;
use crate::filesystem;
use crate::manifest::{
    blueprint_codec,
    snapshot::{SnapshotManifest, SnapshotMetadata, SnapshotProductMetadata},
};

use crate::repo::paths::GutchRepoPaths;
use crate::repo::paths::validate_path_component;

pub(crate) const SNAPSHOT_METADATA_FILE: &str = "snapshot_metadata.json";
pub(crate) const SNAPSHOT_PRODUCT_METADATA_FILE: &str = "snapshot_product_metadata.json";
pub(crate) const SNAPSHOT_BLUEPRINT_FILE: &str = "snapshot_blueprint.gtcbin";

/// Compare installed-tree content and supported properties without reading payload artifacts.
pub fn diff_repo_snapshots(
    repo_root: &std::path::Path,
    from: &str,
    to: &str,
    control: &crate::progress::GutchOperationControl,
) -> Result<crate::snapshot::diff::SnapshotDiff> {
    crate::repo::coordination::read_with_control(repo_root, control, || {
        let (paths, _) = crate::repo::paths::load_gutch_repo_manifest(repo_root)?;
        let before = load_snapshot_manifest(&paths, from)?;
        control.checkpoint()?;
        let after = load_snapshot_manifest(&paths, to)?;
        crate::snapshot::diff::compare_snapshot_entries(&before.entries, &after.entries, control)
    })
}

pub(crate) fn validate_snapshot_id(snapshot_id: &str) -> Result<()> {
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

pub fn snapshot_metadata_path(paths: &GutchRepoPaths, snapshot_id: &str) -> Result<PathBuf> {
    validate_path_component(snapshot_id, "snapshot ID")?;
    Ok(paths
        .snapshots_root
        .join(snapshot_id)
        .join(SNAPSHOT_METADATA_FILE))
}

pub(crate) fn snapshot_blueprint_path(
    paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<PathBuf> {
    validate_path_component(snapshot_id, "snapshot ID")?;
    Ok(paths
        .snapshots_root
        .join(snapshot_id)
        .join(SNAPSHOT_BLUEPRINT_FILE))
}

pub(crate) fn snapshot_product_metadata_path(
    paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<PathBuf> {
    validate_path_component(snapshot_id, "snapshot ID")?;
    Ok(paths
        .snapshots_root
        .join(snapshot_id)
        .join(SNAPSHOT_PRODUCT_METADATA_FILE))
}

pub fn load_snapshot_manifest(
    paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<SnapshotManifest> {
    let metadata_path = snapshot_metadata_path(paths, snapshot_id)?;
    let metadata_json = filesystem::read_to_string(&metadata_path)?;
    let mut manifest = load_snapshot_metadata_from_json_text(snapshot_id, &metadata_json)?;
    manifest.product_metadata = load_snapshot_product_metadata(paths, snapshot_id)?;
    let blueprint_path = snapshot_blueprint_path(paths, snapshot_id)?;
    if !blueprint_path.is_file() {
        return Err(crate::Error::manifest(format!(
            "snapshot blueprint not found at {}",
            blueprint_path.display()
        )));
    }
    manifest.entries = blueprint_codec::read_snapshot_blueprint_binary(&blueprint_path)?;
    validate_snapshot_identity(&manifest)?;
    manifest.total_logical_size = manifest.files().map(|file| file.size).sum();
    manifest.sync_launch_from_product_metadata();
    if let Some(launch) = manifest.launch.as_ref() {
        crate::manifest::launch::validate_resolved_launch_for_snapshot(launch, &manifest)?;
    }
    Ok(manifest)
}

pub(crate) fn snapshot_manifest_from_parts(
    mut metadata: SnapshotManifest,
    product_metadata: Option<SnapshotProductMetadata>,
    blueprint: &[u8],
) -> Result<SnapshotManifest> {
    metadata.entries = blueprint_codec::read_snapshot_blueprint_bytes(blueprint)?;
    validate_snapshot_identity(&metadata)?;
    metadata.total_logical_size = metadata.files().map(|file| file.size).sum();
    metadata.product_metadata = product_metadata;
    metadata.sync_launch_from_product_metadata();
    if let Some(product) = metadata.product_metadata.as_ref() {
        product.validate_for_snapshot(&metadata)?;
    }
    Ok(metadata)
}

fn validate_snapshot_identity(manifest: &SnapshotManifest) -> Result<()> {
    let actual_id = crate::snapshot::identity::compute_snapshot_id_from_entries(&manifest.entries);
    if actual_id != manifest.snapshot_id {
        return Err(crate::Error::SnapshotIntegrityMismatch {
            snapshot_id: manifest.snapshot_id.clone(),
        });
    }
    Ok(())
}

pub(crate) fn serialize_snapshot_metadata_for_remote(
    snapshot_manifest: &SnapshotManifest,
) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(&snapshot_manifest.metadata()).map_err(Into::into)
}

pub(crate) fn serialize_snapshot_product_metadata_for_remote(
    snapshot_manifest: &SnapshotManifest,
) -> Result<Option<Vec<u8>>> {
    let mut metadata = snapshot_manifest.clone();
    metadata.snapshot_tags = metadata.snapshot_tags.normalized()?;
    metadata.sync_product_metadata_from_launch();
    let Some(product_metadata) = metadata.product_metadata.as_ref() else {
        return Ok(None);
    };
    product_metadata.validate_structure()?;
    serde_json::to_vec_pretty(product_metadata)
        .map(Some)
        .map_err(Into::into)
}

pub(crate) fn load_snapshot_product_metadata(
    paths: &GutchRepoPaths,
    snapshot_id: &str,
) -> Result<Option<SnapshotProductMetadata>> {
    let metadata_path = snapshot_product_metadata_path(paths, snapshot_id)?;
    if !metadata_path.is_file() {
        return Ok(None);
    }
    let metadata: SnapshotProductMetadata =
        filesystem::from_json_str(&filesystem::read_to_string(&metadata_path)?)?;
    metadata.validate_structure()?;
    Ok(Some(metadata))
}

pub(crate) fn write_snapshot_product_metadata(
    paths: &GutchRepoPaths,
    snapshot_id: &str,
    metadata: Option<&SnapshotProductMetadata>,
) -> Result<Option<(PathBuf, u64)>> {
    let metadata_path = snapshot_product_metadata_path(paths, snapshot_id)?;
    if let Some(metadata) = metadata {
        metadata.validate_structure()?;
        filesystem::create_dir_all(&paths.snapshots_root.join(snapshot_id))?;
        let bytes = serde_json::to_vec_pretty(metadata)?;
        filesystem::write(&metadata_path, &bytes)?;
        let size = std::fs::metadata(&metadata_path)?.len();
        return Ok(Some((metadata_path, size)));
    }
    if metadata_path.exists() {
        filesystem::remove_file(&metadata_path)?;
    }
    Ok(None)
}

pub fn save_snapshot_product_metadata_for_snapshots(
    repo_root: &std::path::Path,
    snapshot_ids: &[String],
    product_metadata: Option<SnapshotProductMetadata>,
) -> Result<()> {
    let operation_root = (repo_root).to_path_buf();
    crate::repo::coordination::write(&operation_root, || {
        validate_snapshot_product_metadata_for_snapshots(
            repo_root,
            snapshot_ids,
            product_metadata.as_ref(),
        )?;
        if snapshot_ids.is_empty() {
            return Err(crate::Error::manifest(
                "at least one snapshot id is required to save product metadata",
            ));
        }
        let paths = GutchRepoPaths::from_repo_root(repo_root);
        let mut manifests = Vec::with_capacity(snapshot_ids.len());
        for snapshot_id in snapshot_ids {
            let mut snapshot_manifest = load_snapshot_manifest(&paths, snapshot_id)?;
            snapshot_manifest.product_metadata = product_metadata.clone();
            manifests.push(snapshot_manifest);
        }
        for snapshot_manifest in manifests {
            write_snapshot_product_metadata(
                &paths,
                &snapshot_manifest.snapshot_id,
                snapshot_manifest.product_metadata.as_ref(),
            )?;
        }
        Ok(())
    })
}

pub fn validate_snapshot_product_metadata_for_snapshots(
    repo_root: &std::path::Path,
    snapshot_ids: &[String],
    product_metadata: Option<&SnapshotProductMetadata>,
) -> Result<()> {
    if snapshot_ids.is_empty() {
        return Err(crate::Error::manifest(
            "at least one snapshot id is required to validate product metadata",
        ));
    }
    if let Some(metadata) = product_metadata {
        metadata.validate_structure()?;
    }
    let paths = GutchRepoPaths::from_repo_root(repo_root);
    for snapshot_id in snapshot_ids {
        let snapshot_manifest = load_snapshot_manifest(&paths, snapshot_id)?;
        if let Some(metadata) = product_metadata {
            metadata.validate_for_snapshot(&snapshot_manifest)?;
        }
    }
    Ok(())
}

pub(crate) fn write_snapshot_manifest_artifacts(
    paths: &GutchRepoPaths,
    snapshot_manifest: &SnapshotManifest,
) -> Result<(PathBuf, u64)> {
    snapshot_manifest.snapshot_tags.normalized()?;
    crate::manifest::validate_snapshot_entries(&snapshot_manifest.entries)?;
    if paths.repo_metadata_path.exists() {
        let (_, repository) = crate::repo::paths::load_gutch_repo_manifest(&paths.repo_root)?;
        let constraints = repository.constraints.unwrap_or_default();
        constraints.validate_snapshot_tags(
            &snapshot_manifest.snapshot_tags,
            &snapshot_manifest.snapshot_id,
        )?;
        crate::filesystem::constraints::validate_paths(
            &constraints,
            snapshot_manifest.entries.iter().map(|entry| entry.path()),
            "publish snapshot",
            Some(&snapshot_manifest.snapshot_id),
        )?;
    }
    if let Some(product) = snapshot_manifest.product_metadata.as_ref() {
        product.validate_for_snapshot(snapshot_manifest)?;
    }
    filesystem::create_dir_all(&paths.snapshots_root)?;
    let snapshot_id = &snapshot_manifest.snapshot_id;
    crate::repo::paths::validate_path_component(snapshot_id, "snapshot ID")?;
    let staging_dir = tempfile::Builder::new()
        .prefix(".gutch-snapshot-")
        .tempdir_in(&paths.snapshots_root)?;
    let staging_path = staging_dir.path().to_path_buf();

    // snapshot_metadata.json owns snapshot identity, timestamps, and tags.
    // snapshot_product_metadata.json owns runtime data, while
    // snapshot_blueprint.gtcbin owns file and chunk records.
    let mut metadata = snapshot_manifest.clone();
    metadata.snapshot_tags = metadata.snapshot_tags.normalized()?;
    metadata.sync_product_metadata_from_launch();
    let product_metadata = metadata.product_metadata.clone();
    let metadata_json = serde_json::to_vec_pretty(&metadata.metadata())?;
    filesystem::write(&staging_path.join(SNAPSHOT_METADATA_FILE), &metadata_json)?;
    if let Some(product_metadata) = product_metadata.as_ref() {
        product_metadata.validate_structure()?;
        filesystem::write(
            &staging_path.join(SNAPSHOT_PRODUCT_METADATA_FILE),
            &serde_json::to_vec_pretty(product_metadata)?,
        )?;
    }
    blueprint_codec::write_snapshot_blueprint_binary(
        &staging_path.join(SNAPSHOT_BLUEPRINT_FILE),
        &snapshot_manifest.blueprint(),
    )?;
    let staging_path = staging_dir.keep();
    let snapshot_dir = paths.snapshots_root.join(snapshot_id);
    filesystem::directory::replace_directory(&staging_path, &snapshot_dir)?;

    let metadata_path = snapshot_metadata_path(paths, snapshot_id)?;
    let metadata_size = std::fs::metadata(&metadata_path)?.len();
    Ok((metadata_path, metadata_size))
}

pub(crate) fn load_snapshot_metadata_from_json_text(
    snapshot_id: &str,
    metadata_json: &str,
) -> Result<SnapshotManifest> {
    let mut metadata: SnapshotMetadata = serde_json::from_str(metadata_json)?;
    metadata.snapshot_id = snapshot_id.to_string();
    Ok(SnapshotManifest::from_metadata(metadata))
}

pub(crate) fn snapshot_manifest_from_download(
    snapshot_id: &str,
    manifest_bytes: &[u8],
) -> Result<crate::manifest::snapshot::SnapshotManifest> {
    let metadata_json = std::str::from_utf8(manifest_bytes).map_err(|err| {
        crate::Error::manifest(format!("snapshot metadata is not valid UTF-8: {}", err))
    })?;
    crate::repo::snapshot_manifests::load_snapshot_metadata_from_json_text(
        snapshot_id,
        metadata_json,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rejects_blueprints_with_a_different_installation_id() {
        let root = tempfile::tempdir().unwrap();
        let manifest = SnapshotManifest::new(
            crate::snapshot::identity::compute_snapshot_id_from_entries(&[]),
            Default::default(),
        );
        let path = root.path().join("blueprint");
        blueprint_codec::write_snapshot_blueprint_binary(&path, &manifest.blueprint()).unwrap();
        let bytes = std::fs::read(path).unwrap();
        assert!(snapshot_manifest_from_parts(manifest.clone(), None, &bytes).is_ok());
        let mut incorrect = manifest;
        incorrect.snapshot_id = "a".repeat(64);
        assert!(matches!(
            snapshot_manifest_from_parts(incorrect, None, &bytes).unwrap_err(),
            crate::Error::SnapshotIntegrityMismatch { snapshot_id } if snapshot_id == "a".repeat(64)
        ));
    }

    #[test]
    fn rejects_unsafe_snapshot_path_components() {
        let paths = GutchRepoPaths::from_repo_root(Path::new("repo"));
        assert!(snapshot_metadata_path(&paths, "../escape").is_err());
        assert!(snapshot_blueprint_path(&paths, "nested/snapshot").is_err());
        assert!(snapshot_product_metadata_path(&paths, "").is_err());
    }
}
