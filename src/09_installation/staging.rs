//! Staging directories and the path guards that keep destination, staging and
//! repository apart.

use std::fs;
use std::path::{Path, PathBuf};

use crate::filesystem;
use crate::filesystem::directory::lock_directory_promotion;
use crate::filesystem::directory::recover_directory_promotion;
use crate::filesystem::paths::validate_destination_paths;
use crate::filesystem::paths::{require_disjoint_paths, resolve_operation_path};
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::{Error, Result};
use std::fs::File;

pub(crate) fn default_snapshot_staging_dir(destination_dir: &Path, snapshot_id: &str) -> PathBuf {
    let destination_name = destination_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("destination");
    destination_dir.with_file_name(format!(
        ".gutch-staging-{}-{}",
        destination_name, snapshot_id
    ))
}

pub(crate) struct SnapshotStaging {
    directory: tempfile::TempDir,
    created_parent: Option<PathBuf>,
}

impl SnapshotStaging {
    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }
}

impl Drop for SnapshotStaging {
    fn drop(&mut self) {
        let _ = filesystem::remove_dir_all(self.directory.path());
        if let Some(parent) = &self.created_parent {
            // Remove only an empty parent created by this operation.
            let _ = fs::remove_dir(parent);
        }
    }
}

pub(crate) fn create_snapshot_staging(
    destination: &Path,
    staging_parent: &Path,
    repo_root: &Path,
    source: Option<&Path>,
) -> Result<SnapshotStaging> {
    require_disjoint_paths(staging_parent, destination)?;
    if let Some(source) = source {
        require_disjoint_paths(staging_parent, source)?;
    }
    let resolved_staging = resolve_operation_path(staging_parent)?;
    let resolved_repo = resolve_operation_path(repo_root)?;
    if resolved_repo.starts_with(resolve_operation_path(destination)?) {
        return Err(Error::InvalidPath {
            path: destination.to_path_buf(),
            reason: "destination must not contain the repository".into(),
        });
    }
    if resolved_repo.starts_with(&resolved_staging) {
        return Err(Error::InvalidPath {
            path: staging_parent.to_path_buf(),
            reason: "staging must not contain the repository".into(),
        });
    }
    let created_parent = (!staging_parent.exists()).then(|| staging_parent.to_path_buf());
    filesystem::create_dir_all(staging_parent)?;
    let directory = tempfile::Builder::new()
        .prefix(".gutch-operation-")
        .tempdir_in(staging_parent)
        .map_err(|e| {
            Error::io_context(
                "creating reconstruction staging directory",
                staging_parent,
                e,
            )
        })?;
    Ok(SnapshotStaging {
        directory,
        created_parent,
    })
}

pub(crate) fn prepare_installation_destination(
    snapshot_manifest: &SnapshotManifest,
    destination: &Path,
    staging: Option<&Path>,
    repository_path: &Path,
    control: &OperationControl,
) -> Result<File> {
    validate_destination_paths(
        destination,
        snapshot_manifest.entries.iter().map(|entry| entry.path()),
    )?;
    validate_installation_paths(destination, staging, repository_path)?;
    let lock = lock_directory_promotion(destination, control)?;
    recover_directory_promotion(destination)?;
    Ok(lock)
}

pub(crate) fn validate_installation_paths(
    destination: &Path,
    staging: Option<&Path>,
    repository_path: &Path,
) -> Result<()> {
    if let Some(staging) = staging {
        require_disjoint_paths(staging, destination)?;
    }
    let resolved_destination = resolve_operation_path(destination)?;
    let repository = resolve_operation_path(repository_path)?;
    if repository.starts_with(&resolved_destination) {
        return Err(Error::invalid_input(
            "destination must not contain the repository",
        ));
    }
    for owned in [
        "bundles",
        "chunks",
        "snapshots",
        ".gutch-transactions",
        "repo_metadata.json",
        "chunk_map.gtcbin",
        "local_repo_config.json",
        ".gutch-lock",
    ] {
        require_disjoint_paths(destination, &repository_path.join(owned))?;
    }
    Ok(())
}
