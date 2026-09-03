use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingDestinationPathKind {
    File,
    Directory,
    FileOrDirectory,
}

/// Resolves manifest paths beneath one root while reusing the root's canonical path.
///
/// The per-component checks are intentionally retained: an existing symlinked
/// ancestor must still be rejected even when several paths share the same root.
#[derive(Debug, Clone)]
pub(crate) struct ManifestPathResolver {
    root: PathBuf,
    canonical_root: Option<PathBuf>,
    validated_ancestors: Arc<Mutex<HashSet<PathBuf>>>,
}

impl ManifestPathResolver {
    pub(crate) fn new(root: &Path) -> Result<Self> {
        let canonical_root = if root.exists() {
            Some(root.canonicalize().map_err(|error| Error::PathError {
                path: root.to_path_buf(),
                reason: format!("failed to resolve snapshot root: {error}"),
            })?)
        } else {
            None
        };
        Ok(Self {
            root: root.to_path_buf(),
            canonical_root,
            validated_ancestors: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    pub(crate) fn resolve(&self, relative: &str) -> Result<PathBuf> {
        validate_manifest_relative_path(relative, false)?;
        let candidate = self.root.join(relative);
        if let Some(canonical_root) = self.canonical_root.as_deref() {
            ensure_existing_ancestors_stay_within_root(
                &self.root,
                &candidate,
                canonical_root,
                &self.validated_ancestors,
            )?;
        }
        Ok(candidate)
    }

    /// Resolves an entry that the caller will inspect without following its final component.
    /// Existing parents remain canonicalized and containment-checked, while the leaf is left for
    /// `symlink_metadata`, `read_link`, or an equivalent no-follow operation.
    pub(crate) fn resolve_entry(&self, relative: &str) -> Result<PathBuf> {
        validate_manifest_relative_path(relative, false)?;
        let candidate = self.root.join(relative);
        if let (Some(canonical_root), Some(parent)) =
            (self.canonical_root.as_deref(), candidate.parent())
        {
            ensure_existing_ancestors_stay_within_root(
                &self.root,
                parent,
                canonical_root,
                &self.validated_ancestors,
            )?;
        }
        Ok(candidate)
    }
}

pub(crate) fn validate_manifest_relative_path(relative: &str, allow_root: bool) -> Result<()> {
    let trimmed = relative.trim();
    let relative_path = Path::new(relative);
    let has_windows_drive_prefix = trimmed.as_bytes().get(1) == Some(&b':');
    if relative == "." {
        return if allow_root {
            Ok(())
        } else {
            Err(Error::PathError {
                path: relative_path.to_path_buf(),
                reason: "manifest path must identify an entry beneath the snapshot root".into(),
            })
        };
    }
    if trimmed.is_empty()
        || trimmed != relative
        || relative.contains('\0')
        || relative.contains('\\')
        || has_windows_drive_prefix
        || relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_)
                    | Component::RootDir
                    | Component::ParentDir
                    | Component::CurDir
            )
        })
    {
        return Err(Error::PathError {
            path: relative_path.to_path_buf(),
            reason: format!(
                "manifest path must stay within the snapshot root: '{}'",
                relative
            ),
        });
    }

    if relative.split('/').any(str::is_empty) {
        return Err(Error::PathError {
            path: relative_path.to_path_buf(),
            reason: format!(
                "manifest path must be normalized beneath the snapshot root: '{}'",
                relative
            ),
        });
    }

    Ok(())
}

pub(crate) fn resolve_manifest_relative_path(root: &Path, relative: &str) -> Result<PathBuf> {
    ManifestPathResolver::new(root)?.resolve(relative)
}

fn ensure_existing_ancestors_stay_within_root(
    root: &Path,
    candidate: &Path,
    canonical_root: &Path,
    validated_ancestors: &Mutex<HashSet<PathBuf>>,
) -> Result<()> {
    let mut current = root.to_path_buf();
    let relative = candidate.strip_prefix(root).map_err(|_| Error::PathError {
        path: candidate.to_path_buf(),
        reason: "candidate path is not beneath the snapshot root".into(),
    })?;
    for component in relative.components() {
        current.push(component);
        if !current.exists() {
            break;
        }
        let already_validated = validated_ancestors
            .lock()
            .map_err(|_| Error::PathError {
                path: current.clone(),
                reason: "path validation cache is poisoned".to_string(),
            })?
            .contains(&current);
        if already_validated {
            continue;
        }
        let canonical_current = current.canonicalize().map_err(|error| Error::PathError {
            path: current.clone(),
            reason: format!("failed to resolve snapshot path component: {error}"),
        })?;
        if !canonical_current.starts_with(canonical_root) {
            return Err(Error::PathError {
                path: current,
                reason: "snapshot path resolves outside the snapshot root".into(),
            });
        }
        validated_ancestors
            .lock()
            .map_err(|_| Error::PathError {
                path: current.clone(),
                reason: "path validation cache is poisoned".to_string(),
            })?
            .insert(current.clone());
    }
    Ok(())
}

pub fn resolve_existing_destination_path(
    install_root: &Path,
    relative: &str,
    allow_root: bool,
    expected_kind: ExistingDestinationPathKind,
) -> Result<PathBuf> {
    validate_manifest_relative_path(relative, allow_root)?;

    let canonical_root = install_root
        .canonicalize()
        .map_err(|error| Error::PathError {
            path: install_root.to_path_buf(),
            reason: format!("failed to resolve installation root: {error}"),
        })?;
    let candidate = if relative == "." {
        canonical_root.clone()
    } else {
        canonical_root.join(relative)
    };
    let canonical_candidate = candidate.canonicalize().map_err(|error| Error::PathError {
        path: candidate.clone(),
        reason: format!("declared installation path does not exist: {error}"),
    })?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return Err(Error::PathError {
            path: candidate,
            reason: "declared installation path resolves outside the installation root".into(),
        });
    }

    let valid_kind = match expected_kind {
        ExistingDestinationPathKind::File => canonical_candidate.is_file(),
        ExistingDestinationPathKind::Directory => canonical_candidate.is_dir(),
        ExistingDestinationPathKind::FileOrDirectory => {
            canonical_candidate.is_file() || canonical_candidate.is_dir()
        }
    };
    if !valid_kind {
        return Err(Error::PathError {
            path: canonical_candidate,
            reason: format!("declared installation path is not a {:?}", expected_kind),
        });
    }

    Ok(canonical_candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rejects_cross_platform_escape_forms() {
        for path in [
            "../outside",
            "bin/../outside",
            "C:/outside",
            "\\\\server\\share",
            "/outside",
        ] {
            assert!(
                validate_manifest_relative_path(path, false).is_err(),
                "accepted {path}"
            );
        }
    }

    #[test]
    fn allows_install_root_only_when_requested() {
        assert!(validate_manifest_relative_path(".", false).is_err());
        assert!(validate_manifest_relative_path(".", true).is_ok());
    }

    #[test]
    fn entry_resolution_accepts_a_missing_leaf() {
        let base = crate::test_util::temp_path("gutch_entry_path_resolution");
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let resolver = ManifestPathResolver::new(&root).unwrap();

        assert_eq!(
            resolver.resolve_entry("missing.txt").unwrap(),
            root.join("missing.txt")
        );

        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_parent_that_escapes_root() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("gutch-path-guard-{}", std::process::id()));
        let root = base.join("root");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("escape")).unwrap();

        assert!(resolve_manifest_relative_path(&root, "escape/game").is_err());
        let _ = fs::remove_dir_all(base);
    }

    #[cfg(windows)]
    #[test]
    fn rejects_directory_link_that_escapes_root_when_links_are_available() {
        use std::os::windows::fs::symlink_dir;

        let base = std::env::temp_dir().join(format!("gutch-path-guard-{}", std::process::id()));
        let root = base.join("root");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        if symlink_dir(&outside, root.join("escape")).is_ok() {
            assert!(resolve_manifest_relative_path(&root, "escape/game").is_err());
        }
        let _ = fs::remove_dir_all(base);
    }
}
