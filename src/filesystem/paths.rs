use crate::Error;
use crate::Result;
use std::collections::{HashMap, HashSet};
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use unicode_normalization::UnicodeNormalization;
pub(crate) const MAX_MANIFEST_PATH_BYTES: usize = 128 * 1024;
pub(crate) const MAX_MANIFEST_PATH_COMPONENTS: usize = 256;
/// Encode separators structurally; never replace bytes inside a filename.
pub(crate) fn manifest_path_from_native(path: &Path) -> Result<String> {
    let parts = path
        .components()
        .map(|part| match part {
            Component::Normal(name) => name.to_str().ok_or_else(|| Error::PathError {
                path: path.to_path_buf(),
                reason: "filename is not valid Unicode; lossless capture is unavailable".into(),
            }),
            _ => Err(Error::manifest(
                "source path must be relative and normalized",
            )),
        })
        .collect::<Result<Vec<_>>>()?;
    let relative = parts.join("/");
    validate_manifest_relative_path(&relative, false)?;
    Ok(relative)
}
/// Validate output names before creating staging or touching installed content.
pub(crate) fn validate_destination_paths<'a>(
    root: &Path,
    paths: impl Iterator<Item = &'a str>,
) -> Result<()> {
    validate_destination_paths_inner(root, paths).map_err(|error| match error {
        Error::PathError { path, reason } => {
            Error::PathValidation(super::constraints::PathValidationFailure {
                operation: "installation".into(),
                snapshot_id: None,
                violations: vec![super::constraints::PathViolation {
                    path: path
                        .strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned(),
                    conflicting_path: None,
                    target: "actual-destination".into(),
                    code: "destination_incompatible".into(),
                    reason,
                }],
            })
        }
        error => error,
    })
}
fn validate_destination_paths_inner<'a>(
    root: &Path,
    paths: impl Iterator<Item = &'a str>,
) -> Result<()> {
    let absolute = std::path::absolute(root)?;
    // Probe the actual volume: Windows directories and macOS volumes may be case-sensitive.
    let parent = absolute
        .ancestors()
        .find(|p| p.is_dir())
        .ok_or_else(|| Error::manifest("destination has no existing parent"))?;
    let probe = tempfile::Builder::new()
        .prefix(".gutch-path-probe-")
        .tempdir_in(parent)?;
    std::fs::write(probe.path().join("Case"), b"")?;
    std::fs::write(probe.path().join("é"), b"")?;
    let fold_case = probe.path().join("case").exists();
    let fold_normalization = probe.path().join("e\u{301}").exists();
    let mut spellings = HashMap::<String, Vec<String>>::new();
    for relative in paths {
        validate_manifest_relative_path(relative, false)?;
        validate_destination_length(&absolute.join(relative))?;
        let mut prefix = String::new();
        for part in relative.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let key = if fold_normalization {
                prefix.nfd().collect::<String>()
            } else {
                prefix.clone()
            };
            let key = if fold_case { key.to_uppercase() } else { key };
            let previous_names = spellings.entry(key).or_default();
            for previous in previous_names
                .iter()
                .filter(|previous| *previous != &prefix)
            {
                // Uppercasing is only a candidate filter. Ask the filesystem before
                // rejecting names such as sharp-s versus SS on Windows.
                let comparison = tempfile::tempdir_in(probe.path())?;
                std::fs::write(
                    comparison.path().join(previous.rsplit('/').next().unwrap()),
                    b"",
                )?;
                if comparison.path().join(part).exists() {
                    return Err(Error::PathError {
                        path: absolute.join(relative),
                        reason: format!(
                            "destination aliases distinct paths {previous:?} and {prefix:?}"
                        ),
                    });
                }
            }
            if !previous_names.contains(&prefix) {
                previous_names.push(prefix.clone());
            }
        }
    }
    Ok(())
}
fn validate_destination_length(path: &Path) -> Result<()> {
    let fail = |reason: &str| Error::PathError {
        path: path.to_path_buf(),
        reason: reason.into(),
    };
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        if path.as_os_str().encode_wide().count() > 32766 {
            return Err(fail("destination path exceeds 32766 UTF-16 code units"));
        }
        for part in path.components().filter_map(|c| {
            if let Component::Normal(n) = c {
                Some(n)
            } else {
                None
            }
        }) {
            if part.encode_wide().count() > 255 {
                return Err(fail("destination component exceeds 255 UTF-16 code units"));
            }
            let name = part
                .to_str()
                .ok_or_else(|| fail("destination name is not valid Unicode"))?;
            if let Some(reason) = super::constraints::windows_name_reason(name) {
                return Err(fail(reason));
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let max_path = if cfg!(target_os = "macos") {
            1023
        } else {
            4095
        };
        if path.as_os_str().as_bytes().len() > max_path {
            return Err(fail("destination path exceeds platform path budget"));
        }
        for part in path.components().filter_map(|c| {
            if let Component::Normal(n) = c {
                Some(n)
            } else {
                None
            }
        }) {
            if part.as_bytes().len() > 255 {
                return Err(fail("destination component exceeds 255 bytes"));
            }
        }
    }
    Ok(())
}
/// Resolve existing components before comparing paths, including links and `..`.
pub(crate) fn resolve_operation_path(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                resolved.pop();
            }
            _ => {
                resolved.push(component);
                match std::fs::symlink_metadata(&resolved) {
                    Ok(_) => resolved = resolved.canonicalize()?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    #[cfg(windows)]
    let resolved = PathBuf::from(resolved.as_os_str().to_string_lossy().to_lowercase());
    Ok(resolved)
}
pub(crate) fn require_disjoint_paths(left: &Path, right: &Path) -> Result<()> {
    let resolved_left = resolve_operation_path(left)?;
    let resolved_right = resolve_operation_path(right)?;
    if resolved_left.starts_with(&resolved_right) || resolved_right.starts_with(&resolved_left) {
        return Err(Error::PathError {
            path: left.to_path_buf(),
            reason: format!(
                "operation paths must not overlap: '{}' and '{}'",
                left.display(),
                right.display()
            ),
        });
    }
    Ok(())
}
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
    if relative.len() > MAX_MANIFEST_PATH_BYTES
        || relative.split('/').count() > MAX_MANIFEST_PATH_COMPONENTS
    {
        return Err(Error::PathError {
            path: PathBuf::from(relative),
            reason: "manifest path exceeds 128 KiB or 256 components".into(),
        });
    }
    let relative_path = Path::new(relative);
    let has_windows_drive_prefix = relative.as_bytes().get(1) == Some(&b':');
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
    if relative.is_empty()
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
    if relative
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
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
        // Successful components already passed containment validation for this
        // resolver. Consult that cache before issuing another filesystem query.
        // Missing components are never cached, so later creations are checked.
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
        if !current.exists() {
            break;
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
    #[cfg(any(unix, windows))]
    #[test]
    fn resolver_checks_new_links_beneath_a_cached_parent() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("root");
        let parent = root.join("parent");
        let outside = base.path().join("outside");
        fs::create_dir_all(&parent).unwrap();
        fs::create_dir(&outside).unwrap();
        let resolver = ManifestPathResolver::new(&root).unwrap();
        // Cache the existing parent, but never the missing child.
        assert!(resolver.resolve("parent/child/file").is_ok());
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, parent.join("child")).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&outside, parent.join("child")).is_err() {
            return; // Windows may lack the privilege to create symbolic links.
        }
        assert!(resolver.resolve("parent/child/file").is_err());
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
pub(crate) fn normalize_link_target(relative_path: &str, target: &Path) -> Result<String> {
    let link_path = Path::new(relative_path);
    if target.is_absolute() {
        return Err(Error::PathError {
            path: link_path.to_path_buf(),
            reason: "absolute symbolic-link target".into(),
        });
    }
    let combined = Path::new(relative_path)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(target);
    let mut normalized = PathBuf::new();
    for component in combined.components() {
        match component {
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if normalized.pop() => {}
            _ => {
                return Err(Error::PathError {
                    path: link_path.to_path_buf(),
                    reason: "symbolic-link target escapes root".into(),
                });
            }
        }
    }
    Ok(normalized.to_string_lossy().replace('\\', "/"))
}
#[cfg(test)]
mod link_target_tests {
    use super::*;
    #[test]
    fn normalizes_targets_without_allowing_root_escape() {
        assert_eq!(
            normalize_link_target("bin/link", Path::new("../lib/./target")).unwrap(),
            "lib/target"
        );
        assert!(normalize_link_target("link", Path::new("../outside")).is_err());
        assert!(normalize_link_target("bin/link", Path::new("../../outside")).is_err());
        assert!(normalize_link_target("bin/link", &std::env::current_dir().unwrap()).is_err());
    }
}
#[cfg(test)]
mod limit_tests {
    use super::*;
    #[test]
    fn path_limits_count_utf8_bytes_and_components() {
        let allowed = "é".repeat(MAX_MANIFEST_PATH_BYTES / 2);
        assert!(validate_manifest_relative_path(&allowed, false).is_ok());
        assert!(validate_manifest_relative_path(&(allowed + "x"), false).is_err());
        let allowed = vec!["a"; MAX_MANIFEST_PATH_COMPONENTS].join("/");
        assert!(validate_manifest_relative_path(&allowed, false).is_ok());
        assert!(validate_manifest_relative_path(&(allowed + "/a"), false).is_err());
        assert!(validate_manifest_relative_path("a/./b", false).is_err());
    }
    #[test]
    fn destination_rejects_long_components_before_creating_output() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("output");
        assert!(
            validate_destination_paths(&destination, ["a".repeat(255)].iter().map(String::as_str))
                .is_ok()
        );
        assert!(
            validate_destination_paths(&destination, ["a".repeat(256)].iter().map(String::as_str))
                .is_err()
        );
        assert!(!destination.exists());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
    #[cfg(windows)]
    #[test]
    fn windows_aliases_and_reserved_names_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["CON", "nul.txt", "COM¹", "a.", "a ", "a:b", "x?y"] {
            assert!(
                validate_destination_paths(temp.path(), [name].into_iter()).is_err(),
                "{name}"
            );
        }
        assert!(validate_destination_paths(temp.path(), ["dir/a", "DIR/b"].into_iter()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn capture_rejects_non_unicode_and_literal_backslash_names() {
        use std::os::unix::ffi::OsStrExt;
        assert!(
            manifest_path_from_native(Path::new(std::ffi::OsStr::from_bytes(b"bad\xff"))).is_err()
        );
        assert!(manifest_path_from_native(Path::new("literal\\name")).is_err());
        assert_eq!(
            manifest_path_from_native(Path::new("é/e\u{301}")).unwrap(),
            "é/e\u{301}"
        );
    }
}
