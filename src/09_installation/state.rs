//! Resolves the stored snapshot state an operation targets.

use crate::Error;
use crate::Result;
use crate::artifact::hash::hash_hex;
use crate::installation::contracts::ResolvedSnapshotOperationState;
use crate::manifest::current_host_tags;
use crate::repo::paths::load_repo_manifest;
use crate::repo::snapshot_manifests::load_snapshot_manifest;
use crate::repo::snapshot_manifests::serialize_snapshot_metadata_for_remote;
use std::path::Path;

pub fn resolve_snapshot_state(
    repo_root: &Path,
    snapshot_id: &str,
) -> Result<ResolvedSnapshotOperationState> {
    let (repo_paths, _repo_manifest) = load_repo_manifest(repo_root)?;
    let snapshot_manifest = load_snapshot_manifest(&repo_paths, snapshot_id)?;

    let launch = snapshot_manifest.launch.clone();

    let manifest_bytes = serialize_snapshot_metadata_for_remote(&snapshot_manifest)?;
    let snapshot_metadata_hash_b3 = hash_hex(&blake3::hash(&manifest_bytes));

    if snapshot_manifest.snapshot_id.trim().is_empty() {
        return Err(Error::manifest("snapshot manifest is missing snapshot_id"));
    }

    Ok(ResolvedSnapshotOperationState {
        snapshot_id: snapshot_manifest.snapshot_id,
        snapshot_metadata_hash_b3,
        launch,
        external_product_dependencies: snapshot_manifest
            .product_metadata
            .as_ref()
            .map(|metadata| metadata.external_dependencies_for_host_tags(&current_host_tags()))
            .unwrap_or_default(),
    })
}
