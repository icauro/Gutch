use std::path::Path;

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
