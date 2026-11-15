//! Listing of stored snapshots and artifact files.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::Result;

use crate::artifact::hash::ChunkHash;
use crate::artifact::reference::ArtifactRef;
use crate::filesystem::directory::is_owned_promotion_backup;
use crate::repo::paths::RepoPaths;
use crate::repo::paths::bundle_id_from_bundle_path;
use crate::repo::snapshot_manifests::validate_snapshot_id;
use std::fs;

pub(crate) fn list_repo_snapshot_ids(paths: &RepoPaths) -> Result<Vec<String>> {
    if !paths.snapshots_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut snapshot_ids = Vec::new();
    for entry in fs::read_dir(&paths.snapshots_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if is_owned_promotion_backup(&entry.path()) {
            continue;
        }
        let id = entry
            .file_name()
            .to_str()
            .ok_or_else(|| crate::Error::repository("invalid snapshot directory name"))?
            .to_string();
        validate_snapshot_id(&id)?;
        if !entry
            .path()
            .join(super::snapshot_manifests::SNAPSHOT_METADATA_FILE)
            .is_file()
        {
            return Err(crate::Error::repository(format!(
                "snapshot {id} is missing metadata"
            )));
        }
        snapshot_ids.push(id);
    }
    snapshot_ids.sort();
    Ok(snapshot_ids)
}

pub(crate) fn list_repo_artifact_paths(paths: &RepoPaths) -> Result<Vec<(ArtifactRef, PathBuf)>> {
    let mut artifacts = Vec::new();
    visit_repo_artifacts(paths, |artifact, entry| {
        artifacts.push((artifact, entry.into_path()));
        Ok(())
    })?;
    Ok(artifacts)
}

fn visit_repo_artifacts(
    paths: &RepoPaths,
    mut visit: impl FnMut(ArtifactRef, walkdir::DirEntry) -> Result<()>,
) -> Result<()> {
    if paths.bundles_root.exists() {
        for entry in walkdir::WalkDir::new(&paths.bundles_root).sort_by_file_name() {
            let entry = entry?;
            if !(entry.file_type().is_file()
                || (entry.file_type().is_symlink() && entry.path().is_file()))
            {
                continue;
            }
            if let Some(bundle_id) = bundle_id_from_bundle_path(entry.path()) {
                visit(
                    ArtifactRef::Bundle {
                        id: bundle_id.into(),
                    },
                    entry,
                )?;
            }
        }
    }
    if paths.chunks_root.exists() {
        for entry in walkdir::WalkDir::new(&paths.chunks_root).sort_by_file_name() {
            let entry = entry?;
            if !(entry.file_type().is_file()
                || (entry.file_type().is_symlink() && entry.path().is_file()))
            {
                continue;
            }
            let Some(hash) = entry.file_name().to_str() else {
                continue;
            };
            if let Ok(hash) = ChunkHash::parse_hex(hash) {
                visit(ArtifactRef::LooseChunk { hash }, entry)?;
            }
        }
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct ArtifactInventory {
    pub artifacts: HashSet<ArtifactRef>,
    pub artifact_count: usize,
    pub artifact_bytes: u64,
    pub bundle_count: usize,
    pub bundle_bytes: u64,
}

pub(crate) fn scan_artifact_inventory(paths: &RepoPaths) -> Result<ArtifactInventory> {
    let mut inventory = ArtifactInventory::default();
    visit_repo_artifacts(paths, |artifact, entry| {
        let bytes = entry.metadata()?.len();
        if matches!(artifact, ArtifactRef::Bundle { .. }) {
            inventory.bundle_count += 1;
            inventory.bundle_bytes += bytes;
        }
        inventory.artifacts.insert(artifact);
        inventory.artifact_count += 1;
        inventory.artifact_bytes += bytes;
        Ok(())
    })?;
    Ok(inventory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_counts_bundles_and_loose_chunks_in_one_pass() {
        let root = tempfile::tempdir().unwrap();
        let paths = RepoPaths::from_repo_root(root.path());
        let empty = scan_artifact_inventory(&paths).unwrap();
        assert_eq!(empty.artifact_count, 0);
        for (directory, name, bytes) in [
            (&paths.bundles_root, "a".repeat(64), b"bundle".as_slice()),
            (&paths.chunks_root, "b".repeat(64), b"chunk".as_slice()),
            (
                &paths.bundles_root,
                "ignored".to_string(),
                b"other".as_slice(),
            ),
        ] {
            std::fs::create_dir_all(directory).unwrap();
            std::fs::write(directory.join(name), bytes).unwrap();
        }
        let inventory = scan_artifact_inventory(&paths).unwrap();
        assert_eq!(inventory.bundle_count, 1);
        assert_eq!(inventory.bundle_bytes, 6);
        assert_eq!(inventory.artifact_count, 2);
        assert_eq!(inventory.artifact_bytes, 11);
        assert_eq!(inventory.artifacts.len(), 2);
    }

    #[test]
    fn snapshot_listing_excludes_only_owned_promotion_backups() {
        let root = tempfile::tempdir().unwrap();
        let paths = RepoPaths::from_repo_root(root.path());
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
