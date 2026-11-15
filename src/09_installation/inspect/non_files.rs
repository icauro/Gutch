//! Directory and symlink checks for inventoried or individually resolved entries.
use super::inventory::{FilesystemInventory, InventoryEntryKind};
use super::types::{RepairClassification, RepairIssue};
use crate::Result;
use crate::filesystem::paths::normalize_link_target;
use crate::manifest::SnapshotEntry;
use crate::manifest::snapshot::SnapshotManifest;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

/// What exists on disk at the path of a manifest directory or symlink.
enum FoundEntry<'a> {
    Nothing,
    Directory { read_only: bool },
    Symlink { path: &'a Path },
    Other,
}

pub(super) fn classify_inventory_non_files(
    snapshot_manifest: &SnapshotManifest,
    inventory: &FilesystemInventory,
    classification: &mut RepairClassification,
) -> Result<()> {
    for entry in &snapshot_manifest.entries {
        let Some(manifest_path) = non_file_path(entry) else {
            continue;
        };
        let found = match inventory.entries.get(&inventory.key(manifest_path)) {
            None => FoundEntry::Nothing,
            Some(actual) => match actual.kind {
                InventoryEntryKind::Directory => FoundEntry::Directory {
                    read_only: actual
                        .metadata
                        .as_ref()
                        .expect("inventory entries carry metadata")
                        .permissions()
                        .readonly(),
                },
                InventoryEntryKind::Symlink => FoundEntry::Symlink { path: &actual.path },
                InventoryEntryKind::File | InventoryEntryKind::Other => FoundEntry::Other,
            },
        };
        if let Some(issue) = check_non_file(entry, found)? {
            classification.push(issue);
        }
    }
    Ok(())
}

pub(super) fn classify_resolved_non_files(
    snapshot_manifest: &SnapshotManifest,
    resolved_paths: &[PathBuf],
    classification: &mut RepairClassification,
) -> Result<()> {
    for (entry, path) in snapshot_manifest.entries.iter().zip(resolved_paths) {
        if non_file_path(entry).is_none() {
            continue;
        }
        let found = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_dir() => FoundEntry::Directory {
                read_only: metadata.permissions().readonly(),
            },
            Ok(metadata) if metadata.file_type().is_symlink() => FoundEntry::Symlink { path },
            Ok(_) => FoundEntry::Other,
            Err(error) if error.kind() == ErrorKind::NotFound => FoundEntry::Nothing,
            Err(error) => return Err(error.into()),
        };
        if let Some(issue) = check_non_file(entry, found)? {
            classification.push(issue);
        }
    }
    Ok(())
}

fn non_file_path(entry: &SnapshotEntry) -> Option<&str> {
    match entry {
        SnapshotEntry::File(_) => None,
        SnapshotEntry::Directory(directory) => Some(&directory.path),
        SnapshotEntry::Symlink(link) => Some(&link.path),
    }
}

/// Compares a manifest directory or symlink with what was found on disk.
fn check_non_file(entry: &SnapshotEntry, found: FoundEntry<'_>) -> Result<Option<RepairIssue>> {
    let issue = match (entry, found) {
        (SnapshotEntry::File(_), _) => None,
        (SnapshotEntry::Directory(directory), FoundEntry::Directory { read_only }) => {
            (read_only != directory.read_only).then(|| {
                RepairIssue::corrupt(
                    &directory.path,
                    format!("read-only mismatch for directory: {}", directory.path),
                )
            })
        }
        (SnapshotEntry::Directory(directory), FoundEntry::Nothing) => Some(RepairIssue::missing(
            &directory.path,
            format!("missing directory: {}", directory.path),
        )),
        (SnapshotEntry::Directory(directory), _) => Some(RepairIssue::corrupt(
            &directory.path,
            format!("entry type mismatch for directory: {}", directory.path),
        )),
        (SnapshotEntry::Symlink(link), FoundEntry::Symlink { path }) => {
            let actual_target = normalize_link_target(&link.path, &fs::read_link(path)?)?;
            (actual_target != link.target_path).then(|| {
                RepairIssue::corrupt(
                    &link.path,
                    format!("symbolic-link target mismatch for {}", link.path),
                )
            })
        }
        (SnapshotEntry::Symlink(link), FoundEntry::Nothing) => Some(RepairIssue::missing(
            &link.path,
            format!("missing symbolic link: {}", link.path),
        )),
        (SnapshotEntry::Symlink(link), _) => Some(RepairIssue::corrupt(
            &link.path,
            format!("entry type mismatch for symbolic link: {}", link.path),
        )),
    };
    Ok(issue)
}
