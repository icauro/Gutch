//! Physical inventory and symlink-parent classification for inspection.
use crate::filesystem::paths::ManifestPathResolver;
use crate::filesystem::paths::validate_manifest_relative_path;
use crate::manifest::SnapshotEntry;
use crate::manifest::snapshot::SnapshotManifest;
use crate::progress::OperationControl;
use crate::{Result, manifest};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::Metadata;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InventoryEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug)]
pub(super) struct FilesystemInventoryEntry {
    pub(super) path: PathBuf,
    pub(super) kind: InventoryEntryKind,
    pub(super) metadata: Option<Metadata>,
}

#[derive(Debug)]
pub(super) struct FilesystemInventory {
    pub(super) canonical_root: PathBuf,
    pub(super) entries: HashMap<String, FilesystemInventoryEntry>,
    pub(super) extra_paths: Vec<String>,
}

pub(super) fn build_filesystem_inventory(
    snapshot_manifest: &SnapshotManifest,
    dir: &Path,
    control: &OperationControl,
) -> Result<FilesystemInventory> {
    let expected_paths: Vec<&str> = snapshot_manifest
        .entries
        .iter()
        .map(SnapshotEntry::path)
        .collect();
    for path in &expected_paths {
        validate_manifest_relative_path(path, false)?;
    }
    let canonical_root = match dir.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(FilesystemInventory {
                canonical_root: dir.to_path_buf(),
                entries: HashMap::new(),
                extra_paths: Vec::new(),
            });
        }
        Err(error) => {
            return Err(crate::Error::InvalidPath {
                path: dir.to_path_buf(),
                reason: format!("failed to resolve snapshot root: {error}"),
            });
        }
    };

    let expected_path_keys = expected_paths
        .iter()
        .map(|path| inventory_path_key(path))
        .collect::<HashSet<_>>();
    let mut entries = HashMap::with_capacity(expected_path_keys.len());
    let mut extra_paths = Vec::new();
    for entry in walkdir::WalkDir::new(&canonical_root)
        .follow_links(false)
        .sort_by_file_name()
    {
        control.checkpoint()?;
        let entry = entry?;
        if entry.depth() == 0 {
            continue;
        }
        let Ok(relative_path) = entry.path().strip_prefix(&canonical_root) else {
            continue;
        };
        let relative = relative_path.to_string_lossy().replace('\\', "/");
        let kind = if entry.file_type().is_symlink() {
            InventoryEntryKind::Symlink
        } else if entry.file_type().is_file() {
            InventoryEntryKind::File
        } else if entry.file_type().is_dir() {
            InventoryEntryKind::Directory
        } else {
            InventoryEntryKind::Other
        };
        if kind == InventoryEntryKind::Directory {
            let canonical_entry =
                entry
                    .path()
                    .canonicalize()
                    .map_err(|error| crate::Error::InvalidPath {
                        path: entry.path().to_path_buf(),
                        reason: format!("failed to resolve snapshot path component: {error}"),
                    })?;
            if !canonical_entry.starts_with(&canonical_root) {
                return Err(crate::Error::InvalidPath {
                    path: entry.path().to_path_buf(),
                    reason: "snapshot path resolves outside the snapshot root".into(),
                });
            }
        }
        let key = inventory_path_key(&relative);
        let metadata = if expected_path_keys.contains(&key) {
            Some(entry.metadata()?)
        } else {
            extra_paths.push(relative.clone());
            None
        };
        entries.insert(
            key,
            FilesystemInventoryEntry {
                path: entry.into_path(),
                kind,
                metadata,
            },
        );
    }
    Ok(FilesystemInventory {
        canonical_root,
        entries,
        extra_paths,
    })
}

pub(super) fn augment_inventory_for_symlinked_parents(
    snapshot_manifest: &SnapshotManifest,
    dir: &Path,
    inventory: &mut FilesystemInventory,
    control: &OperationControl,
) -> Result<()> {
    let needs_fallback = snapshot_manifest.entries.iter().any(|entry| {
        !inventory
            .entries
            .contains_key(&inventory_path_key(entry.path()))
            && inventory_has_symlink_parent(entry.path(), inventory)
    });
    if !needs_fallback {
        return Ok(());
    }
    let path_resolver = ManifestPathResolver::new(dir)?;
    for entry in &snapshot_manifest.entries {
        control.checkpoint()?;
        let key = inventory_path_key(entry.path());
        if inventory.entries.contains_key(&key)
            || !inventory_has_symlink_parent(entry.path(), inventory)
        {
            continue;
        }
        let path = match entry {
            manifest::SnapshotEntry::Symlink(_) => path_resolver.resolve(entry.path())?,
            manifest::SnapshotEntry::File(_) | manifest::SnapshotEntry::Directory(_) => {
                path_resolver.resolve_entry(entry.path())?
            }
        };
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let kind = if metadata.file_type().is_symlink() {
            InventoryEntryKind::Symlink
        } else if metadata.file_type().is_file() {
            InventoryEntryKind::File
        } else if metadata.file_type().is_dir() {
            InventoryEntryKind::Directory
        } else {
            InventoryEntryKind::Other
        };
        inventory.entries.insert(
            key,
            FilesystemInventoryEntry {
                path,
                kind,
                metadata: Some(metadata),
            },
        );
    }
    Ok(())
}

pub(super) fn inventory_has_symlink_parent(
    relative: &str,
    inventory: &FilesystemInventory,
) -> bool {
    let mut parent = String::new();
    let mut components = relative.split('/').peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            break;
        }
        if !parent.is_empty() {
            parent.push('/');
        }
        parent.push_str(component);
        if inventory
            .entries
            .get(&inventory_path_key(&parent))
            .is_some_and(|entry| entry.kind == InventoryEntryKind::Symlink)
        {
            return true;
        }
    }
    false
}

pub(super) fn inventory_path_key(relative: &str) -> String {
    #[cfg(windows)]
    {
        relative.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        relative.to_string()
    }
}
