//! Physical inventory and symlink-parent classification for inspection.
use crate::filesystem;
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
    pub(super) name_case: NameCase,
    pub(super) entries: HashMap<String, FilesystemInventoryEntry>,
    pub(super) extra_paths: Vec<String>,
}

impl FilesystemInventory {
    /// The `entries` key for a manifest-relative path.
    pub(super) fn key(&self, relative: &str) -> String {
        self.name_case.key(relative)
    }
}

/// Whether the inspected tree tells names apart by letter case. This belongs
/// to the filesystem (and on some systems the folder), not the OS, so it is
/// probed in the tree itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NameCase {
    Sensitive,
    Insensitive,
}

impl NameCase {
    pub(super) fn detect(root: &Path) -> Self {
        probe_name_case(root)
            .or_else(|| root.parent().and_then(probe_name_case))
            .unwrap_or(if cfg!(any(windows, target_os = "macos")) {
                Self::Insensitive
            } else {
                Self::Sensitive
            })
    }

    pub(super) fn key(self, relative: &str) -> String {
        match self {
            Self::Sensitive => relative.to_string(),
            Self::Insensitive => fold_case(relative),
        }
    }
}

/// Per-character uppercase mapping, like the NTFS and APFS case tables;
/// characters whose uppercase form is several characters (`ß`) stay as is.
fn fold_case(text: &str) -> String {
    text.chars()
        .map(|character| single_char(character.to_uppercase()).unwrap_or(character))
        .collect()
}

fn single_char(mut mapped: impl Iterator<Item = char>) -> Option<char> {
    match (mapped.next(), mapped.next()) {
        (Some(character), None) => Some(character),
        _ => None,
    }
}

/// The name with every cased letter flipped, or `None` if nothing changes.
fn swap_case(name: &str) -> Option<String> {
    let swapped: String = name
        .chars()
        .map(|character| {
            let mapped = if character.is_lowercase() {
                single_char(character.to_uppercase())
            } else {
                single_char(character.to_lowercase())
            };
            mapped.unwrap_or(character)
        })
        .collect();
    (swapped != name).then_some(swapped)
}

/// Looks up one entry of `dir` under a case-swapped spelling. If that spelling
/// exists without being listed separately, the folder ignores case.
fn probe_name_case(dir: &Path) -> Option<NameCase> {
    let names = fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect::<HashSet<_>>();
    names.iter().find_map(|name| {
        let swapped = swap_case(name)?;
        if names.contains(&swapped) {
            return Some(NameCase::Sensitive);
        }
        match fs::symlink_metadata(dir.join(&swapped)) {
            Ok(_) => Some(NameCase::Insensitive),
            Err(error) if error.kind() == ErrorKind::NotFound => Some(NameCase::Sensitive),
            Err(_) => None,
        }
    })
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
    let name_case = NameCase::detect(dir);
    let canonical_root = match dir.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(FilesystemInventory {
                canonical_root: dir.to_path_buf(),
                name_case,
                entries: HashMap::new(),
                extra_paths: Vec::new(),
            });
        }
        Err(error) => {
            return Err(crate::Error::invalid_path(
                dir,
                format!("failed to resolve snapshot root: {error}"),
            ));
        }
    };

    let expected_path_keys = expected_paths
        .iter()
        .map(|path| name_case.key(path))
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
            let canonical_entry = entry.path().canonicalize().map_err(|error| {
                crate::Error::invalid_path(
                    entry.path(),
                    format!("failed to resolve snapshot path component: {error}"),
                )
            })?;
            if !canonical_entry.starts_with(&canonical_root) {
                return Err(crate::Error::invalid_path(
                    entry.path(),
                    "snapshot path resolves outside the snapshot root",
                ));
            }
        }
        let key = name_case.key(&relative);
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
        name_case,
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
        !inventory.entries.contains_key(&inventory.key(entry.path()))
            && inventory_has_symlink_parent(entry.path(), inventory)
    });
    if !needs_fallback {
        return Ok(());
    }
    let path_resolver = ManifestPathResolver::new(dir)?;
    for entry in &snapshot_manifest.entries {
        control.checkpoint()?;
        let key = inventory.key(entry.path());
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
        let Some(metadata) = filesystem::symlink_metadata_if_exists(&path)? else {
            continue;
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
            .get(&inventory.key(&parent))
            .is_some_and(|entry| entry.kind == InventoryEntryKind::Symlink)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
