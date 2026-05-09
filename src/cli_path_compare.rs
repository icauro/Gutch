use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gutch as g;
use serde::Serialize;

use crate::cli_reporting::OptionalOutputPaths;

#[derive(Debug, Serialize)]
pub struct PathCompareStats {
    pub command: &'static str,
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
    pub outputs: OptionalOutputPaths,
}

pub fn compare_paths(
    expected: &Path,
    actual: &Path,
    outputs: OptionalOutputPaths,
) -> g::Result<PathCompareStats> {
    let expected_files = collect_comparable_files(expected)?;
    let actual_files = collect_comparable_files(actual)?;
    let actual_file_map: HashMap<String, PathBuf> = actual_files.into_iter().collect();
    let mut matched_files = Vec::new();
    let mut mismatched_files = Vec::new();
    let mut missing_files = Vec::new();
    let mut seen_actual_relative_paths = HashSet::new();

    for (relative_path, expected_file_path) in &expected_files {
        let actual_file_path = resolve_actual_file_path(expected, actual, relative_path)?;
        if !actual_file_path.exists() || !actual_file_path.is_file() {
            missing_files.push(relative_path.clone());
            continue;
        }

        let expected_metadata = std::fs::metadata(expected_file_path)?;
        let actual_metadata = std::fs::metadata(&actual_file_path)?;
        let is_match = if expected_metadata.len() != actual_metadata.len() {
            false
        } else {
            let expected_hash = g::hash::hash_file(expected_file_path)?;
            let actual_hash = g::hash::hash_file(&actual_file_path)?;
            expected_hash == actual_hash
        };

        if is_match {
            matched_files.push(relative_path.clone());
        } else {
            mismatched_files.push(relative_path.clone());
        }
        seen_actual_relative_paths.insert(relative_path.clone());
    }

    let mut extra_files = Vec::new();
    if actual.is_dir() {
        for actual_relative_path in actual_file_map.keys() {
            if !seen_actual_relative_paths.contains(actual_relative_path)
                && !expected_files.iter().any(|(expected_relative_path, _)| {
                    expected_relative_path == actual_relative_path
                })
            {
                extra_files.push(actual_relative_path.clone());
            }
        }
    }

    extra_files.sort();
    mismatched_files.sort();
    missing_files.sort();

    let compare_passed =
        mismatched_files.is_empty() && missing_files.is_empty() && extra_files.is_empty();
    Ok(PathCompareStats {
        command: "path compare",
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
        outputs,
    })
}

fn collect_comparable_files(root_path: &Path) -> g::Result<Vec<(String, PathBuf)>> {
    if root_path.is_file() {
        let file_name = root_path.file_name().ok_or_else(|| g::Error::PathError {
            path: root_path.to_path_buf(),
            reason: "file path has no file name".to_string(),
        })?;
        return Ok(vec![(
            file_name.to_string_lossy().to_string(),
            root_path.to_path_buf(),
        )]);
    }

    if root_path.is_dir() {
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(root_path).sort_by_file_name() {
            let entry = entry?;
            if !entry.path().is_file() {
                continue;
            }
            let relative_path = entry
                .path()
                .strip_prefix(root_path)
                .map_err(|err| g::Error::PathError {
                    path: entry.path().to_path_buf(),
                    reason: err.to_string(),
                })?
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative_path, entry.path().to_path_buf()));
        }
        return Ok(files);
    }

    Err(g::Error::PathError {
        path: root_path.to_path_buf(),
        reason: "path does not exist or is unsupported".to_string(),
    })
}

fn resolve_actual_file_path(
    expected_root: &Path,
    actual_root: &Path,
    relative_path: &str,
) -> g::Result<PathBuf> {
    if expected_root.is_file() && actual_root.is_file() {
        return Ok(actual_root.to_path_buf());
    }

    if actual_root.is_dir() {
        return Ok(actual_root.join(relative_path));
    }

    Err(g::Error::PathError {
        path: actual_root.to_path_buf(),
        reason: "actual path must be a directory when expected path is a directory".to_string(),
    })
}
