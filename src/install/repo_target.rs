use std::path::{Path, PathBuf};

use crate::manifest::target::TargetManifest;
use crate::{GutchRepoPaths, Result};

pub(super) fn load_repo_target_manifest(
    repo_root: &Path,
    target_id: &str,
) -> Result<(GutchRepoPaths, TargetManifest)> {
    let (repo_paths, _) = crate::load_gutch_repo_manifest(repo_root)?;
    let target_manifest = crate::load_target_manifest(&repo_paths, target_id)?;
    Ok((repo_paths, target_manifest))
}

pub(super) fn load_repo_target_bundle_paths(
    repo_root: &Path,
    target_id: &str,
) -> Result<(GutchRepoPaths, TargetManifest, Vec<PathBuf>)> {
    let (repo_paths, target_manifest) = load_repo_target_manifest(repo_root, target_id)?;
    let bundle_paths =
        super::apply::resolve_required_bundle_paths_from_repo(&repo_paths, &target_manifest)?;
    Ok((repo_paths, target_manifest, bundle_paths))
}
