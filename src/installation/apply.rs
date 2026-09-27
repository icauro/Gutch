use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::manifest::snapshot::SnapshotManifest;
use crate::runtime::RuntimeResourceHints;

use super::types::InstallationReconstructResult;
use super::types::InstallationRequest;
use super::types::InstallationVerifyRequest;
use super::types::InstallationVerifyResult;
use super::types::SnapshotReconstructResult;
use super::types::SnapshotReconstructSource;
use crate::Error;
use crate::Result;
use crate::installation::inspect::VerifyRequest;
use crate::installation::inspect::classify_directory;

pub(crate) struct VerifiedSource<'a> {
    manifest: SnapshotManifest,
    directory: &'a Path,
}
impl VerifiedSource<'_> {
    pub(crate) fn source(&self) -> SnapshotReconstructSource<'_> {
        SnapshotReconstructSource {
            snapshot_manifest: &self.manifest,
            destination_dir: self.directory,
        }
    }
}

pub(crate) fn resolve_verified_source<'a>(
    source: Option<SnapshotReconstructSource<'a>>,
    runtime_hints: &RuntimeResourceHints,
    control: &crate::progress::GutchOperationControl,
) -> Result<Option<VerifiedSource<'a>>> {
    control.checkpoint()?;
    let Some(source) = source else {
        return Ok(None);
    };
    let (classification, _) = classify_directory(VerifyRequest {
        snapshot_manifest: source.snapshot_manifest,
        dir: source.destination_dir,
        mode: crate::installation::inspect::VerifyMode::Fast,
        runtime_hints,
        control: control.clone(),
    })?;
    let invalid: HashSet<_> = classification
        .missing_files
        .iter()
        .chain(classification.corrupt_files.iter())
        .map(|issue| issue.path.as_str())
        .collect();
    let mut manifest = source.snapshot_manifest.clone();
    manifest
        .entries
        .retain(|entry| !invalid.contains(entry.path()));
    manifest.total_logical_size = manifest.files().map(|file| file.size).sum();
    Ok(Some(VerifiedSource {
        manifest,
        directory: source.destination_dir,
    }))
}

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
        let _ = crate::filesystem::remove_dir_all(self.directory.path());
        if let Some(parent) = &self.created_parent {
            // Remove only an empty parent created by this operation.
            let _ = std::fs::remove_dir(parent);
        }
    }
}

pub(crate) fn create_snapshot_staging(
    destination: &Path,
    staging_parent: &Path,
    repo_root: &Path,
    source: Option<&Path>,
) -> Result<SnapshotStaging> {
    crate::filesystem::paths::require_disjoint_paths(staging_parent, destination)?;
    if let Some(source) = source {
        crate::filesystem::paths::require_disjoint_paths(staging_parent, source)?;
    }
    let resolved_staging = crate::filesystem::paths::resolve_operation_path(staging_parent)?;
    let resolved_repo = crate::filesystem::paths::resolve_operation_path(repo_root)?;
    if resolved_repo.starts_with(crate::filesystem::paths::resolve_operation_path(
        destination,
    )?) {
        return Err(Error::PathError {
            path: destination.to_path_buf(),
            reason: "destination must not contain the repository".into(),
        });
    }
    if resolved_repo.starts_with(&resolved_staging) {
        return Err(Error::PathError {
            path: staging_parent.to_path_buf(),
            reason: "staging must not contain the repository".into(),
        });
    }
    let created_parent = (!staging_parent.exists()).then(|| staging_parent.to_path_buf());
    crate::filesystem::create_dir_all(staging_parent)?;
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

pub fn verify_installation(request: InstallationVerifyRequest) -> Result<InstallationVerifyResult> {
    request.control.checkpoint()?;
    let root = request.repo_path.clone();
    crate::repo::coordination::read_with_control(&root, &request.control.clone(), || {
        request.control.checkpoint()?;
        let super::prepare::LoadedRepoSnapshot {
            repo_paths,
            snapshot_manifest,
            ..
        } = super::prepare::load_repo_snapshot_manifest(&request.repo_path, &request.snapshot_id)?;
        let (classification, verify_timings) = classify_directory(VerifyRequest {
            snapshot_manifest: &snapshot_manifest,
            dir: &request.destination_dir,
            mode: request.verify_mode,
            runtime_hints: &request.runtime_hints,
            control: request.control,
        })?;

        Ok(InstallationVerifyResult {
            repo_root: repo_paths.repo_root,
            snapshot_manifest,
            classification,
            verify_timings,
        })
    })
}

pub fn reconstruct_snapshot(request: InstallationRequest) -> Result<InstallationReconstructResult> {
    super::install::install_snapshot_from_repo(
        request,
        super::install::InstallationPublication::ReplaceDirectory,
    )
    .map(|result| result.reconstruction)
}

pub(crate) struct BuildSnapshotReconstructResultRequest<'a> {
    pub snapshot_manifest: &'a SnapshotManifest,
    pub destination_dir: &'a Path,
    pub staging_dir: &'a Path,
    pub reused_source_chunk_count: usize,
    pub required_chunk_count: usize,
    pub required_bundle_count: usize,
    pub required_artifact_count: usize,
    pub reconstruct_timings: super::types::ReconstructTimings,
    pub verify_timings: crate::installation::inspect::VerifyTimings,
}

pub(crate) fn build_snapshot_reconstruct_result(
    request: BuildSnapshotReconstructResultRequest<'_>,
) -> SnapshotReconstructResult {
    SnapshotReconstructResult {
        snapshot_id: request.snapshot_manifest.snapshot_id.clone(),
        destination_dir: request.destination_dir.to_path_buf(),
        staging_dir: request.staging_dir.to_path_buf(),
        reused_source_chunk_count: request.reused_source_chunk_count,
        required_chunk_count: request.required_chunk_count,
        required_bundle_count: request.required_bundle_count,
        required_artifact_count: request.required_artifact_count,
        reconstruct_timings: request.reconstruct_timings,
        verify_timings: request.verify_timings,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub(crate) fn prepare_installation_destination(
    snapshot_manifest: &SnapshotManifest,
    destination: &Path,
    staging: Option<&Path>,
    repository_path: &Path,
    control: &crate::progress::GutchOperationControl,
) -> Result<std::fs::File> {
    crate::filesystem::paths::validate_destination_paths(
        destination,
        snapshot_manifest.entries.iter().map(|entry| entry.path()),
    )?;
    validate_installation_paths(destination, staging, repository_path)?;
    let lock = crate::filesystem::directory::lock_directory_promotion(destination, control)?;
    crate::filesystem::directory::recover_directory_promotion(destination)?;
    Ok(lock)
}

pub(crate) fn validate_installation_paths(
    destination: &Path,
    staging: Option<&Path>,
    repository_path: &Path,
) -> Result<()> {
    if let Some(staging) = staging {
        crate::filesystem::paths::require_disjoint_paths(staging, destination)?;
    }
    let resolved_destination = crate::filesystem::paths::resolve_operation_path(destination)?;
    let repository = crate::filesystem::paths::resolve_operation_path(repository_path)?;
    if repository.starts_with(&resolved_destination) {
        return Err(Error::manifest(
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
        crate::filesystem::paths::require_disjoint_paths(
            destination,
            &repository_path.join(owned),
        )?;
    }
    Ok(())
}
