use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::Result;

use super::{GutchRepoPaths, bundle_id_from_bundle_path};

pub(crate) fn list_repo_snapshot_ids(paths: &GutchRepoPaths) -> Result<Vec<String>> {
    if !paths.snapshots_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut snapshot_ids = std::fs::read_dir(&paths.snapshots_root)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.path().is_dir()
                && entry
                    .path()
                    .join(super::snapshot_manifests::SNAPSHOT_INDEX_FILE)
                    .is_file()
        })
        .filter_map(|entry| {
            entry
                .path()
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    snapshot_ids.sort();
    Ok(snapshot_ids)
}

pub(crate) fn list_repo_bundle_paths(paths: &GutchRepoPaths) -> Vec<PathBuf> {
    if !paths.bundles_root.exists() {
        return Vec::new();
    }

    walkdir::WalkDir::new(&paths.bundles_root)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_file())
        .filter(|entry| bundle_id_from_bundle_path(entry.path()).is_some())
        .map(|entry| entry.path().to_path_buf())
        .collect()
}

pub(crate) fn scan_bundle_inventory(bundles_root: &Path) -> Result<(HashSet<String>, usize, u64)> {
    if !bundles_root.exists() {
        return Ok((HashSet::new(), 0, 0));
    }

    let mut bundle_ids = HashSet::new();
    let mut bundle_count = 0usize;
    let mut bundle_bytes = 0u64;
    for entry in walkdir::WalkDir::new(bundles_root).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }

        if let Some(bundle_id) = bundle_id_from_bundle_path(entry.path()) {
            bundle_count += 1;
            bundle_bytes += entry.metadata()?.len();
            bundle_ids.insert(bundle_id);
        }
    }

    Ok((bundle_ids, bundle_count, bundle_bytes))
}
