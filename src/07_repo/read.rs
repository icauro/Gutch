//! Public repository readers acquire coordination before reading committed state.
use crate::Result;
use crate::manifest::SnapshotManifest;
use crate::manifest::repo_manifest::RepoManifest;
use crate::repo::coordination;
use crate::repo::paths::{self, RepoPaths};
use crate::repo::snapshot_manifests;
use std::path::Path;

/// Load one committed repository metadata document under reader protection.
pub fn load_repo_manifest(repo_root: &Path) -> Result<RepoManifest> {
    coordination::read(repo_root, || {
        paths::load_repo_manifest(repo_root).map(|(_, manifest)| manifest)
    })
}

/// Load a coherent snapshot metadata/blueprint pair under reader protection.
pub fn load_snapshot_manifest(repo_root: &Path, snapshot_id: &str) -> Result<SnapshotManifest> {
    coordination::read(repo_root, || {
        snapshot_manifests::load_snapshot_manifest(&RepoPaths::open(repo_root)?, snapshot_id)
    })
}
