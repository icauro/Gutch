use super::types::ResolvedSnapshotOperationState;
use crate::{Error, Result};

pub fn resolve_snapshot_state(
    repo_root: &std::path::Path,
    snapshot_id: &str,
) -> Result<ResolvedSnapshotOperationState> {
    let (repo_paths, _repo_manifest) = crate::load_gutch_repo_manifest(repo_root)?;
    let snapshot_manifest = crate::load_snapshot_manifest(&repo_paths, snapshot_id)?;

    let launch = snapshot_manifest.launch.clone();

    let manifest_bytes = crate::serialize_snapshot_metadata_for_remote(&snapshot_manifest)?;
    let snapshot_metadata_hash_b3 = crate::hash::hash_hex(&blake3::hash(&manifest_bytes));

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
            .map(|metadata| {
                metadata.external_dependencies_for_host_tags(&crate::manifest::current_host_tags())
            })
            .unwrap_or_default(),
    })
}
