//! Compares two physical directory trees by kind, content and link target.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::artifact::integrity::hash_file;
use crate::filesystem::paths::normalize_link_target;
use serde::Serialize;
use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComparableKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone)]
struct ComparableEntry {
    path: PathBuf,
    kind: ComparableKind,
}

#[derive(Debug, Serialize)]
pub struct PathComparisonResult {
    pub expected_path: String,
    pub actual_path: String,
    pub expected_file_count: usize,
    pub actual_file_count: usize,
    pub matched_file_count: usize,
    pub mismatched_file_count: usize,
    pub missing_file_count: usize,
    pub extra_file_count: usize,
    pub compare_passed: bool,
    pub mismatched_files: Vec<String>,
    pub missing_files: Vec<String>,
    pub extra_files: Vec<String>,
}

pub fn compare_paths(expected: &Path, actual: &Path) -> crate::Result<PathComparisonResult> {
    let expected_files = collect_comparable_files(expected)?;
    let mut actual_files = collect_comparable_files(actual)?;
    if expected.is_file()
        && actual.is_file()
        && expected_files.len() == 1
        && actual_files.len() == 1
    {
        actual_files[0].0 = expected_files[0].0.clone();
    }
    let actual_file_map: HashMap<String, ComparableEntry> = actual_files.into_iter().collect();
    let mut matched_files = Vec::new();
    let mut mismatched_files = Vec::new();
    let mut missing_files = Vec::new();
    let mut seen_actual_relative_paths = HashSet::new();

    for (relative_path, expected_entry) in &expected_files {
        let actual_entry = actual_file_map.get(relative_path);
        let Some(actual_entry) = actual_entry else {
            missing_files.push(relative_path.clone());
            continue;
        };
        let is_match = entries_match(expected_entry, actual_entry, relative_path)?;

        if is_match {
            matched_files.push(relative_path.clone());
        } else {
            mismatched_files.push(relative_path.clone());
        }
        seen_actual_relative_paths.insert(relative_path.clone());
    }

    // Collect extras for every actual shape: a lone actual file whose name matches
    // an entry of an expected directory must not pass as the whole tree.
    let mut extra_files: Vec<String> = actual_file_map
        .keys()
        .filter(|actual_relative_path| !seen_actual_relative_paths.contains(*actual_relative_path))
        .cloned()
        .collect();

    extra_files.sort();
    mismatched_files.sort();
    missing_files.sort();

    // A directory never matches a single file, even when their entry names agree.
    let root_kinds_match = expected.is_dir() == actual.is_dir();
    let compare_passed = root_kinds_match
        && mismatched_files.is_empty()
        && missing_files.is_empty()
        && extra_files.is_empty();
    Ok(PathComparisonResult {
        expected_path: expected.display().to_string(),
        actual_path: actual.display().to_string(),
        expected_file_count: expected_files.len(),
        actual_file_count: actual_file_map.len(),
        matched_file_count: matched_files.len(),
        mismatched_file_count: mismatched_files.len(),
        missing_file_count: missing_files.len(),
        extra_file_count: extra_files.len(),
        compare_passed,
        mismatched_files,
        missing_files,
        extra_files,
    })
}

fn collect_comparable_files(root_path: &Path) -> crate::Result<Vec<(String, ComparableEntry)>> {
    if root_path.is_file() {
        let file_name = root_path
            .file_name()
            .ok_or_else(|| crate::Error::PathError {
                path: root_path.to_path_buf(),
                reason: "file path has no file name".to_string(),
            })?;
        return Ok(vec![(
            file_name.to_string_lossy().to_string(),
            ComparableEntry {
                path: root_path.to_path_buf(),
                kind: ComparableKind::File,
            },
        )]);
    }

    if root_path.is_dir() {
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(root_path).sort_by_file_name() {
            let entry = entry?;
            if entry.depth() == 0 {
                continue;
            }
            let relative_path = entry
                .path()
                .strip_prefix(root_path)
                .map_err(|err| crate::Error::PathError {
                    path: entry.path().to_path_buf(),
                    reason: err.to_string(),
                })?
                .to_string_lossy()
                .replace('\\', "/");
            let kind = if entry.file_type().is_file() {
                ComparableKind::File
            } else if entry.file_type().is_dir() {
                ComparableKind::Directory
            } else if entry.file_type().is_symlink() {
                ComparableKind::Symlink
            } else {
                return Err(crate::Error::PathError {
                    path: entry.path().to_path_buf(),
                    reason: "unsupported filesystem entry".to_string(),
                });
            };
            files.push((
                relative_path,
                ComparableEntry {
                    path: entry.into_path(),
                    kind,
                },
            ));
        }
        return Ok(files);
    }

    Err(crate::Error::PathError {
        path: root_path.to_path_buf(),
        reason: "path does not exist or is unsupported".to_string(),
    })
}

fn entries_match(
    expected: &ComparableEntry,
    actual: &ComparableEntry,
    relative_path: &str,
) -> crate::Result<bool> {
    if expected.kind != actual.kind {
        return Ok(false);
    }
    let expected_metadata = fs::symlink_metadata(&expected.path)?;
    let actual_metadata = fs::symlink_metadata(&actual.path)?;
    if expected_metadata.permissions().readonly() != actual_metadata.permissions().readonly() {
        return Ok(false);
    }
    match expected.kind {
        ComparableKind::Directory => Ok(true),
        ComparableKind::Symlink => Ok(normalized_link_target(relative_path, &expected.path)?
            == normalized_link_target(relative_path, &actual.path)?),
        ComparableKind::File => {
            if expected_metadata.len() != actual_metadata.len() {
                return Ok(false);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if (expected_metadata.permissions().mode() & 0o111)
                    != (actual_metadata.permissions().mode() & 0o111)
                {
                    return Ok(false);
                }
            }
            Ok(hash_file(&expected.path)? == hash_file(&actual.path)?)
        }
    }
}

fn normalized_link_target(relative_path: &str, link_path: &Path) -> crate::Result<String> {
    normalize_link_target(relative_path, &fs::read_link(link_path)?)
}

#[cfg(test)]
#[path = "05_path_compare_tests.rs"]
mod tests;
