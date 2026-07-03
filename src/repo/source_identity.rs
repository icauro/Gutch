use std::path::Path;

use crate::hash::{hash_file, hash_hex};
use crate::manifest::FileManifest;
use crate::{Error, Result};

pub fn compute_target_id_from_directory(dir: &Path) -> Result<String> {
    Ok(compute_target_id_from_files(
        &scan_directory_files_for_content_id(dir)?,
    ))
}

pub fn source_modified_at_from_directory(dir: &Path) -> Result<String> {
    Ok(format_system_time_utc(max_file_modified_time(dir)?))
}

pub(crate) fn format_system_time_utc(time: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

pub(crate) fn is_source_modified_at_newer(candidate: &str, recorded: &str) -> bool {
    if recorded.trim().is_empty() {
        return true;
    }
    let Some(recorded_time) = parse_source_modified_at(recorded) else {
        return true;
    };
    let Some(candidate_time) = parse_source_modified_at(candidate) else {
        return false;
    };
    candidate_time > recorded_time
}

fn parse_source_modified_at(value: &str) -> Option<std::time::SystemTime> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(std::time::SystemTime::from)
}

fn max_file_modified_time(dir: &Path) -> Result<std::time::SystemTime> {
    if !dir.is_dir() {
        return Err(Error::manifest(format!(
            "source path is not a directory: {}",
            dir.display()
        )));
    }

    let mut max_mtime = std::time::SystemTime::UNIX_EPOCH;
    let mut found_file = false;
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }
        let mtime = entry.metadata()?.modified()?;
        if !found_file || mtime > max_mtime {
            max_mtime = mtime;
            found_file = true;
        }
    }

    if found_file {
        return Ok(max_mtime);
    }

    dir.metadata()?.modified().map_err(|err| {
        Error::manifest(format!(
            "failed to read source directory mtime for {}: {err}",
            dir.display()
        ))
    })
}

fn scan_directory_files_for_content_id(dir: &Path) -> Result<Vec<FileManifest>> {
    let mut file_manifests = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }

        let relative = entry
            .path()
            .strip_prefix(dir)
            .map_err(|err| Error::PathError {
                path: entry.path().to_path_buf(),
                reason: err.to_string(),
            })?;
        let normalized_relative = relative.to_string_lossy().replace('\\', "/");
        let metadata = entry.metadata()?;
        let full_file_hash = hash_hex(&hash_file(entry.path())?);
        file_manifests.push(FileManifest {
            path: normalized_relative,
            size: metadata.len(),
            executable: false,
            full_file_hash,
            chunks: Vec::new(),
        });
    }

    Ok(file_manifests)
}

pub fn compute_target_id_from_files(files: &[FileManifest]) -> String {
    let mut sorted_files = files.to_vec();
    sorted_files.sort_by(|left, right| left.path.cmp(&right.path));

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"gutch-target-content-id-v1");
    for file in &sorted_files {
        hasher.update(file.path.as_bytes());
        hasher.update(b"\x00");
        hasher.update(&file.size.to_le_bytes());
        hasher.update(b"\x00");
        hasher.update(&[u8::from(file.executable)]);
        hasher.update(b"\x00");
        hasher.update(file.full_file_hash.as_bytes());
        hasher.update(b"\x00");
    }
    hash_hex(&hasher.finalize())
}
