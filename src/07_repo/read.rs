//! Public repository readers acquire coordination before reading committed state.
use crate::Result;
use crate::manifest::SnapshotManifest;
use crate::manifest::repo_manifest::RepoManifest;
use crate::repo::coordination;
use crate::repo::paths::RepoPaths;
use std::path::Path;

/// Load one committed repository metadata document under reader protection.
pub fn load_gutch_repo_manifest(root: &Path) -> Result<(RepoPaths, RepoManifest)> {
    coordination::read(root, || crate::repo::paths::load_gutch_repo_manifest(root))
}

/// Load a coherent snapshot metadata/blueprint pair under reader protection.
pub fn load_snapshot_manifest(paths: &RepoPaths, snapshot_id: &str) -> Result<SnapshotManifest> {
    coordination::read(&paths.repo_root, || {
        crate::repo::snapshot_manifests::load_snapshot_manifest(paths, snapshot_id)
    })
}
