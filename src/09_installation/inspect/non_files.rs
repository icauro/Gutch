//! Directory and symlink checks for inventoried or individually resolved entries.
use super::inventory::{FilesystemInventory, InventoryEntryKind, inventory_path_key};
use super::types::{RepairClassification, RepairIssue, RepairIssueKind};
use crate::manifest::snapshot::SnapshotManifest;
use crate::{Result, manifest};
use std::path::PathBuf;

pub(super) fn classify_inventory_non_files(
    snapshot_manifest: &SnapshotManifest,
    inventory: &FilesystemInventory,
    classification: &mut RepairClassification,
) -> Result<()> {
    for entry in &snapshot_manifest.entries {
        match entry {
            manifest::SnapshotEntry::File(_) => {}
            manifest::SnapshotEntry::Directory(directory) => {
                match inventory.entries.get(&inventory_path_key(&directory.path)) {
                    Some(actual) if actual.kind == InventoryEntryKind::Directory => {
                        if actual
                            .metadata
                            .as_ref()
                            .expect("expected inventory directory metadata")
                            .permissions()
                            .readonly()
                            != directory.read_only
                        {
                            classification.push(RepairIssue {
                                kind: RepairIssueKind::Corrupt,
                                path: directory.path.clone(),
                                detail: format!(
                                    "read-only mismatch for directory: {}",
                                    directory.path
                                ),
                            });
                        }
                    }
                    Some(_) => classification.push(RepairIssue {
                        kind: RepairIssueKind::Corrupt,
                        path: directory.path.clone(),
                        detail: format!("entry type mismatch for directory: {}", directory.path),
                    }),
                    None => classification.push(RepairIssue {
                        kind: RepairIssueKind::Missing,
                        path: directory.path.clone(),
                        detail: format!("missing directory: {}", directory.path),
                    }),
                }
            }
            manifest::SnapshotEntry::Symlink(link) => {
                match inventory.entries.get(&inventory_path_key(&link.path)) {
                    Some(actual) if actual.kind == InventoryEntryKind::Symlink => {
                        let raw_target = std::fs::read_link(&actual.path)?;
                        let actual_target = crate::filesystem::paths::normalize_link_target(
                            &link.path,
                            &raw_target,
                        )?;
                        if actual_target != link.target_path {
                            classification.push(RepairIssue {
                                kind: RepairIssueKind::Corrupt,
                                path: link.path.clone(),
                                detail: format!("symbolic-link target mismatch for {}", link.path),
                            });
                        }
                    }
                    Some(_) => classification.push(RepairIssue {
                        kind: RepairIssueKind::Corrupt,
                        path: link.path.clone(),
                        detail: format!("entry type mismatch for symbolic link: {}", link.path),
                    }),
                    None => classification.push(RepairIssue {
                        kind: RepairIssueKind::Missing,
                        path: link.path.clone(),
                        detail: format!("missing symbolic link: {}", link.path),
                    }),
                }
            }
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
        match entry {
            manifest::SnapshotEntry::File(_) => {}
            manifest::SnapshotEntry::Directory(directory) => {
                match std::fs::symlink_metadata(path) {
                    Ok(metadata) if metadata.file_type().is_dir() => {
                        if metadata.permissions().readonly() != directory.read_only {
                            classification.push(RepairIssue {
                                kind: RepairIssueKind::Corrupt,
                                path: directory.path.clone(),
                                detail: format!(
                                    "read-only mismatch for directory: {}",
                                    directory.path
                                ),
                            });
                        }
                    }
                    Ok(_) => classification.push(RepairIssue {
                        kind: RepairIssueKind::Corrupt,
                        path: directory.path.clone(),
                        detail: format!("entry type mismatch for directory: {}", directory.path),
                    }),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => classification
                        .push(RepairIssue {
                            kind: RepairIssueKind::Missing,
                            path: directory.path.clone(),
                            detail: format!("missing directory: {}", directory.path),
                        }),
                    Err(error) => return Err(error.into()),
                }
            }
            manifest::SnapshotEntry::Symlink(link) => match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    let raw_target = std::fs::read_link(path)?;
                    let actual_target =
                        crate::filesystem::paths::normalize_link_target(&link.path, &raw_target)?;
                    if actual_target != link.target_path {
                        classification.push(RepairIssue {
                            kind: RepairIssueKind::Corrupt,
                            path: link.path.clone(),
                            detail: format!("symbolic-link target mismatch for {}", link.path),
                        });
                    }
                }
                Ok(_) => classification.push(RepairIssue {
                    kind: RepairIssueKind::Corrupt,
                    path: link.path.clone(),
                    detail: format!("entry type mismatch for symbolic link: {}", link.path),
                }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    classification.push(RepairIssue {
                        kind: RepairIssueKind::Missing,
                        path: link.path.clone(),
                        detail: format!("missing symbolic link: {}", link.path),
                    })
                }
                Err(error) => return Err(error.into()),
            },
        }
    }
    Ok(())
}
