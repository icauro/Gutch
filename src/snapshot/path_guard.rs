use std::path::{Component, Path, PathBuf};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingInstallPathKind {
    File,
    Directory,
    FileOrDirectory,
}

pub fn validate_manifest_relative_path(relative: &str, allow_root: bool) -> Result<()> {
    let trimmed = relative.trim();
    let relative_path = Path::new(relative);
    let has_windows_drive_prefix = trimmed.as_bytes().get(1) == Some(&b':');
    if relative == "." {
        return if allow_root {
            Ok(())
        } else {
            Err(Error::PathError {
                path: relative_path.to_path_buf(),
                reason: "manifest path must identify an entry beneath the target root".into(),
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
                "manifest path must stay within the target root: '{}'",
                relative
            ),
        });
    }

    if relative.split('/').any(str::is_empty) {
        return Err(Error::PathError {
            path: relative_path.to_path_buf(),
            reason: format!(
                "manifest path must be normalized beneath the target root: '{}'",
                relative
            ),
        });
    }

    Ok(())
}

pub(crate) fn resolve_manifest_relative_path(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_manifest_relative_path(relative, false)?;
    let candidate = root.join(relative);
    ensure_existing_ancestors_stay_within_root(root, &candidate)?;
    Ok(candidate)
}

fn ensure_existing_ancestors_stay_within_root(root: &Path, candidate: &Path) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let canonical_root = root.canonicalize().map_err(|error| Error::PathError {
        path: root.to_path_buf(),
        reason: format!("failed to resolve target root: {error}"),
    })?;
    let mut current = root.to_path_buf();
    let relative = candidate.strip_prefix(root).map_err(|_| Error::PathError {
        path: candidate.to_path_buf(),
        reason: "candidate path is not beneath the target root".into(),
    })?;
    for component in relative.components() {
        current.push(component);
        if !current.exists() {
            break;
        }
        let canonical_current = current.canonicalize().map_err(|error| Error::PathError {
            path: current.clone(),
            reason: format!("failed to resolve target path component: {error}"),
        })?;
        if !canonical_current.starts_with(&canonical_root) {
            return Err(Error::PathError {
                path: current,
                reason: "target path resolves outside the target root".into(),
            });
        }
    }
    Ok(())
}

pub fn resolve_existing_install_path(
    install_root: &Path,
    relative: &str,
    allow_root: bool,
    expected_kind: ExistingInstallPathKind,
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
        ExistingInstallPathKind::File => canonical_candidate.is_file(),
        ExistingInstallPathKind::Directory => canonical_candidate.is_dir(),
        ExistingInstallPathKind::FileOrDirectory => {
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
