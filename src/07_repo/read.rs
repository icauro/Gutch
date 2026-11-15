//! Public repository readers acquire coordination before reading committed state.
use crate::Result;

/// Load one committed repository metadata document under reader protection.
pub fn load_gutch_repo_manifest(
    root: &std::path::Path,
) -> Result<(
    crate::repo::paths::GutchRepoPaths,
    crate::manifest::gutch_repo::GutchRepoManifest,
)> {
    crate::repo::coordination::read(root, || crate::repo::paths::load_gutch_repo_manifest(root))
}

/// Load a coherent snapshot metadata/blueprint pair under reader protection.
pub fn load_snapshot_manifest(
    paths: &crate::repo::paths::GutchRepoPaths,
    snapshot_id: &str,
) -> Result<crate::manifest::SnapshotManifest> {
    crate::repo::coordination::read(&paths.repo_root, || {
        crate::repo::snapshot_manifests::load_snapshot_manifest(paths, snapshot_id)
    })
}
