//! Manifest path validation, native/manifest path conversion, and guards that keep operation paths disjoint.

use crate::Error;
use crate::Result;
use crate::error::path_validation::PathValidationFailure;
use crate::error::path_validation::PathViolation;
use crate::error::path_validation::ViolationCode;
use crate::filesystem::constraints::MAX_NAME_LENGTH;
use crate::filesystem::constraints::fold_case;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use unicode_normalization::UnicodeNormalization;
pub(crate) const MAX_MANIFEST_PATH_BYTES: usize = 128 * 1024;
pub(crate) const MAX_MANIFEST_PATH_COMPONENTS: usize = 256;
/// Encode separators structurally; never replace bytes inside a filename.
pub(crate) fn manifest_path_from_native(path: &Path) -> Result<String> {
    let parts = path
        .components()
        .map(|part| match part {
            Component::Normal(name) => name.to_str().ok_or_else(|| Error::InvalidPath {
                path: path.to_path_buf(),
                reason: "filename is not valid Unicode; lossless capture is unavailable".into(),
            }),
            _ => Err(Error::invalid_input(
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
    validate_paths_on_destination_volume(root, paths).map_err(|error| match error {
        Error::InvalidPath { path, reason } => Error::PathValidation(PathValidationFailure {
            operation: "installation".into(),
            snapshot_id: None,
            violations: vec![PathViolation {
                path: path.trim_prefix(root).to_string_lossy().into_owned(),
                conflicting_path: None,
                target: "actual-destination".into(),
                code: ViolationCode::DestinationIncompatible,
                reason,
            }],
        }),
        error => error,
    })
}
fn validate_paths_on_destination_volume<'a>(
    root: &Path,
    paths: impl Iterator<Item = &'a str>,
) -> Result<()> {
    let absolute = path::absolute(root)?;
    // Probe the actual volume: Windows directories and macOS volumes may be case-sensitive.
    let parent = absolute
        .ancestors()
        .find(|p| p.is_dir())
        .ok_or_else(|| Error::invalid_input("destination has no existing parent"))?;
    let probe = tempfile::Builder::new()
        .prefix(".gutch-path-probe-")
        .tempdir_in(parent)?;
    fs::write(probe.path().join("Case"), b"")?;
    fs::write(probe.path().join("é"), b"")?;
    let fold_case_names = probe.path().join("case").exists();
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
            let key = if fold_case_names {
                fold_case(&key)
            } else {
                key
            };
            let parent = &prefix[..prefix.len() - part.len()];
            let previous_names = spellings.entry(key).or_default();
            // Spellings under a different parent were settled one level up: those
            // parents either alias (already rejected) or are distinct directories.
            for previous in previous_names.iter().filter(|previous| {
                *previous != &prefix
                    && previous
                        .strip_prefix(parent)
                        .is_some_and(|name| !name.contains('/'))
            }) {
                // Uppercasing is only a candidate filter. Ask the filesystem before
                // rejecting names such as sharp-s versus SS on Windows.
                let comparison = tempfile::tempdir_in(probe.path())?;
                fs::write(comparison.path().join(&previous[parent.len()..]), b"")?;
                if comparison.path().join(part).exists() {
                    return Err(Error::InvalidPath {
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
    let fail = |reason: &str| Error::InvalidPath {
        path: path.to_path_buf(),
        reason: reason.into(),
    };
    #[cfg(windows)]
    {
        use crate::filesystem::constraints::WINDOWS_MAX_PATH_UNITS;
        use std::os::windows::ffi::OsStrExt;
        if path.as_os_str().encode_wide().count() > WINDOWS_MAX_PATH_UNITS {
            return Err(fail(&format!(
                "destination path exceeds {WINDOWS_MAX_PATH_UNITS} UTF-16 code units"
            )));
        }
        for part in path.components().filter_map(|c| {
            if let Component::Normal(n) = c {
                Some(n)
            } else {
                None
            }
        }) {
            if part.encode_wide().count() > MAX_NAME_LENGTH {
                return Err(fail(&format!(
                    "destination component exceeds {MAX_NAME_LENGTH} UTF-16 code units"
                )));
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
        use crate::filesystem::constraints::{APPLE_MAX_PATH_BYTES, UNIX_MAX_PATH_BYTES};
        use std::os::unix::ffi::OsStrExt;
        let max_path = if cfg!(target_os = "macos") {
            APPLE_MAX_PATH_BYTES
        } else {
            UNIX_MAX_PATH_BYTES
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
            if part.as_bytes().len() > MAX_NAME_LENGTH {
                return Err(fail(&format!(
                    "destination component exceeds {MAX_NAME_LENGTH} bytes"
                )));
            }
        }
    }
    Ok(())
}
/// Resolve existing components before comparing paths, including links and `..`.
pub(crate) fn resolve_operation_path(path: &Path) -> Result<PathBuf> {
    let absolute = path::absolute(path)?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                resolved.pop();
            }
            _ => {
                resolved.push(component);
                match fs::symlink_metadata(&resolved) {
                    Ok(_) => resolved = resolved.canonicalize()?,
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
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
        return Err(Error::InvalidPath {
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
/// Resolves manifest paths beneath one root while reusing the root's canonical path.
///
/// The per-component checks are intentionally retained: an existing symlinked
/// ancestor must still be rejected even when several paths share the same root.
/// Parents that passed once stay trusted for the resolver's life, so build a
/// new resolver after something else may have replaced them. A root missing
/// at construction is never containment-checked.
#[derive(Debug, Clone)]
pub(crate) struct ManifestPathResolver {
    root: PathBuf,
    canonical_root: Option<PathBuf>,
    validated_ancestors: Arc<Mutex<HashSet<PathBuf>>>,
}
impl ManifestPathResolver {
    pub(crate) fn new(root: &Path) -> Result<Self> {
        let canonical_root = if root.exists() {
            Some(root.canonicalize().map_err(|error| Error::InvalidPath {
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
        return Err(Error::InvalidPath {
            path: PathBuf::from(relative),
            reason: format!(
                "manifest path exceeds {} KiB or {MAX_MANIFEST_PATH_COMPONENTS} components",
                MAX_MANIFEST_PATH_BYTES / 1024
            ),
        });
    }
    let relative_path = Path::new(relative);
    let has_windows_drive_prefix = relative.as_bytes().get(1) == Some(&b':');
    if relative == "." {
        return if allow_root {
            Ok(())
        } else {
            Err(Error::InvalidPath {
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
        return Err(Error::InvalidPath {
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
        return Err(Error::InvalidPath {
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
    let relative = candidate
        .strip_prefix(root)
        .map_err(|_| Error::InvalidPath {
            path: candidate.to_path_buf(),
            reason: "candidate path is not beneath the snapshot root".into(),
        })?;
    for component in relative.components() {
        current.push(component);
        // Successful components already passed containment validation for this
        // resolver. Consult that cache before issuing another filesystem query.
        // Missing components are never cached, so later creations are checked.
        // A set cannot be left half-updated, so a poisoned cache is still sound.
        let already_validated = validated_ancestors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&current);
        if already_validated {
            continue;
        }
        // Inspect the link itself: a dangling symlink reports `exists() == false`,
        // yet a later create through it would land wherever the link points.
        match fs::symlink_metadata(&current) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => {
                return Err(Error::io_context("inspecting path", &current, error));
            }
        }
        let canonical_current = current.canonicalize().map_err(|error| Error::InvalidPath {
            path: current.clone(),
            reason: format!("failed to resolve snapshot path component: {error}"),
        })?;
        if !canonical_current.starts_with(canonical_root) {
            return Err(Error::InvalidPath {
                path: current,
                reason: "snapshot path resolves outside the snapshot root".into(),
            });
        }
        validated_ancestors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(current.clone());
    }
    Ok(())
}

pub(crate) fn normalize_link_target(relative_path: &str, target: &Path) -> Result<String> {
    let link_path = Path::new(relative_path);
    if target.is_absolute() {
        return Err(Error::InvalidPath {
            path: link_path.to_path_buf(),
            reason: "absolute symbolic-link target".into(),
        });
    }
    let combined = Path::new(relative_path)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(target);
    let mut normalized = Vec::new();
    for component in combined.components() {
        match component {
            Component::Normal(part) => {
                normalized.push(part.to_str().ok_or_else(|| Error::InvalidPath {
                    path: link_path.to_path_buf(),
                    reason: "symbolic-link target is not valid Unicode".into(),
                })?)
            }
            Component::CurDir => {}
            Component::ParentDir if normalized.pop().is_some() => {}
            _ => {
                return Err(Error::InvalidPath {
                    path: link_path.to_path_buf(),
                    reason: "symbolic-link target escapes root".into(),
                });
            }
        }
    }
    // Join structurally: on Unix a backslash stays part of its filename.
    Ok(normalized.join("/"))
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
    fn disjoint_paths_reject_nesting_in_either_direction() {
        let base = tempfile::tempdir().unwrap();
        let parent = base.path().join("parent");
        let child = parent.join("missing").join("child");
        let sibling = base.path().join("sibling");
        assert!(require_disjoint_paths(&parent, &child).is_err());
        assert!(require_disjoint_paths(&child, &parent).is_err());
        assert!(require_disjoint_paths(&parent, &parent.join(".")).is_err());
        assert!(require_disjoint_paths(&parent, &sibling).is_ok());
        assert!(require_disjoint_paths(&child, &sibling.join("..").join("parent")).is_err());
    }
    #[test]
    fn allows_install_root_only_when_requested() {
        assert!(validate_manifest_relative_path(".", false).is_err());
        assert!(validate_manifest_relative_path(".", true).is_ok());
    }
    #[test]
    fn entry_resolution_accepts_a_missing_leaf() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let resolver = ManifestPathResolver::new(&root).unwrap();
        assert_eq!(
            resolver.resolve_entry("missing.txt").unwrap(),
            root.join("missing.txt")
        );
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
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("root");
        let outside = base.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("escape")).unwrap();
        assert!(resolve_manifest_relative_path(&root, "escape/game").is_err());
    }
    #[cfg(windows)]
    #[test]
    fn rejects_directory_link_that_escapes_root_when_links_are_available() {
        use std::os::windows::fs::symlink_dir;
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("root");
        let outside = base.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        if symlink_dir(&outside, root.join("escape")).is_ok() {
            assert!(resolve_manifest_relative_path(&root, "escape/game").is_err());
        }
    }

    #[test]
    fn rejects_dangling_link_that_would_create_outside_root() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let missing_outside = base.path().join("missing");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&missing_outside, root.join("leaf")).is_ok();
        #[cfg(windows)]
        let linked =
            std::os::windows::fs::symlink_file(&missing_outside, root.join("leaf")).is_ok();
        if linked {
            assert!(resolve_manifest_relative_path(&root, "leaf").is_err());
        }
    }
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
    #[cfg(unix)]
    #[test]
    fn keeps_unix_backslashes_inside_target_names() {
        assert_eq!(
            normalize_link_target("bin/link", Path::new("a\\b")).unwrap(),
            "bin/a\\b"
        );
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
    #[test]
    fn distinct_parents_do_not_make_equal_children_aliases() {
        // Uppercasing folds sharp-s to SS, but common filesystems keep them distinct.
        let temp = tempfile::tempdir().unwrap();
        validate_destination_paths(temp.path(), ["ß/x", "SS/x", "ß/y"].into_iter()).unwrap();
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
