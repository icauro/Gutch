use std::path::Path;

use crate::Result;
use crate::manifest::snapshot::SnapshotManifest;
use crate::repo::paths::GutchRepoPaths;

pub(super) fn load_repo_snapshot_manifest(
    repo_root: &Path,
    snapshot_id: &str,
) -> Result<(GutchRepoPaths, SnapshotManifest)> {
    let (repo_paths, repo_manifest) = crate::repo::paths::load_gutch_repo_manifest(repo_root)?;
    let snapshot_manifest =
        crate::repo::snapshot_manifests::load_snapshot_manifest(&repo_paths, snapshot_id)?;
    crate::filesystem::constraints::validate_paths(
        &repo_manifest.constraints.unwrap_or_default(),
        snapshot_manifest.entries.iter().map(|entry| entry.path()),
        "installation",
        Some(snapshot_id),
    )?;
    Ok((repo_paths, snapshot_manifest))
}
