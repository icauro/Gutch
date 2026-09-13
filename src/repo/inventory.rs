use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::Result;

use crate::repo::paths::GutchRepoPaths;
use crate::repo::paths::bundle_id_from_bundle_path;

pub(crate) fn list_repo_snapshot_ids(paths: &GutchRepoPaths) -> Result<Vec<String>> {
    if !paths.snapshots_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut snapshot_ids = Vec::new();
    for entry in std::fs::read_dir(&paths.snapshots_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if crate::filesystem::directory::is_owned_promotion_backup(&entry.path()) {
            continue;
        }
        let id = entry
            .file_name()
            .to_str()
            .ok_or_else(|| crate::Error::manifest("invalid snapshot directory name"))?
            .to_string();
        crate::repo::snapshot_manifests::validate_snapshot_id(&id)?;
        if !entry
            .path()
            .join(super::snapshot_manifests::SNAPSHOT_METADATA_FILE)
            .is_file()
        {
            return Err(crate::Error::manifest(format!(
                "snapshot {id} is missing metadata"
            )));
        }
        snapshot_ids.push(id);
    }
    snapshot_ids.sort();
    Ok(snapshot_ids)
}

pub(crate) fn list_repo_artifact_paths(
    paths: &GutchRepoPaths,
) -> Result<Vec<(crate::repo::artifacts::ArtifactRef, PathBuf)>> {
    let mut artifacts = Vec::new();
    visit_repo_artifacts(paths, |artifact, entry| {
        artifacts.push((artifact, entry.path().to_path_buf()));
        Ok(())
    })?;
    Ok(artifacts)
}

fn visit_repo_artifacts(
    paths: &GutchRepoPaths,
    mut visit: impl FnMut(crate::repo::artifacts::ArtifactRef, walkdir::DirEntry) -> Result<()>,
) -> Result<()> {
    if paths.bundles_root.exists() {
        for entry in walkdir::WalkDir::new(&paths.bundles_root).sort_by_file_name() {
            let entry = entry?;
            if !entry.path().is_file() {
                continue;
            }
            if let Some(bundle_id) = bundle_id_from_bundle_path(entry.path()) {
                visit(
                    crate::repo::artifacts::ArtifactRef::Bundle { id: bundle_id },
                    entry,
                )?;
            }
        }
    }
    if paths.chunks_root.exists() {
        for entry in walkdir::WalkDir::new(&paths.chunks_root).sort_by_file_name() {
            let entry = entry?;
            if !entry.path().is_file() {
                continue;
            }
            let Some(hash) = entry.file_name().to_str() else {
                continue;
            };
            if crate::artifact::hash::ChunkHash::parse_hex(hash).is_ok() {
                let artifact = crate::repo::artifacts::ArtifactRef::LooseChunk {
                    hash: hash.to_string(),
                };
                visit(artifact, entry)?;
            }
        }
    }
    Ok(())
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

pub(crate) fn scan_artifact_inventory(
    paths: &GutchRepoPaths,
) -> Result<(HashSet<crate::repo::artifacts::ArtifactRef>, usize, u64)> {
    let mut artifacts = HashSet::new();
    let mut artifact_count = 0usize;
    let mut artifact_bytes = 0u64;

    visit_repo_artifacts(paths, |artifact, entry| {
        artifacts.insert(artifact);
        artifact_count += 1;
        artifact_bytes += entry.metadata()?.len();
        Ok(())
    })?;

    Ok((artifacts, artifact_count, artifact_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_listing_excludes_only_owned_promotion_backups() {
        let root = tempfile::tempdir().unwrap();
        let paths = GutchRepoPaths::from_repo_root(root.path());
        let snapshot_id = "a".repeat(64);
        let snapshot = paths.snapshots_root.join(&snapshot_id);
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(snapshot.join("snapshot_metadata.json"), b"{}").unwrap();
        let backup = paths
            .snapshots_root
            .join(format!(".gutch-backup-{}", "b".repeat(64)));
        std::fs::create_dir(&backup).unwrap();
        assert!(list_repo_snapshot_ids(&paths).is_err());
        std::fs::write(backup.with_extension("owner"), b"GUTCH_PROMOTION_V1").unwrap();
        assert_eq!(list_repo_snapshot_ids(&paths).unwrap(), vec![snapshot_id]);
        std::fs::write(backup.with_extension("owner"), b"unowned").unwrap();
        assert!(list_repo_snapshot_ids(&paths).is_err());
    }
}
