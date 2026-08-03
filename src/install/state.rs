use super::types::ResolvedTargetInstallState;
use crate::manifest::launch::resolve_repo_launch_for_target;
use crate::{Error, Result};

pub fn resolve_target_install_state(
    repo_root: &std::path::Path,
    target_id: &str,
) -> Result<ResolvedTargetInstallState> {
    let (repo_paths, repo_manifest) = crate::load_gutch_repo_manifest(repo_root)?;
    let target_manifest = crate::load_target_manifest(&repo_paths, target_id)?;

    let launch = match target_manifest.launch.clone() {
        Some(launch) => Some(launch),
        None => repo_manifest
            .launch
            .as_ref()
            .map(|configuration| resolve_repo_launch_for_target(configuration, &target_manifest))
            .transpose()?,
    };

    let manifest_bytes = crate::serialize_target_manifest_for_remote(&target_manifest)?;
    let manifest_hash = crate::hash::hash_hex(&blake3::hash(&manifest_bytes));

    if target_manifest.target_id.trim().is_empty() {
        return Err(Error::manifest("target manifest is missing target_id"));
    }

    Ok(ResolvedTargetInstallState {
        target_id: target_manifest.target_id,
        manifest_hash,
        launch,
        external_product_dependencies: repo_manifest.external_product_dependencies,
    })
}
