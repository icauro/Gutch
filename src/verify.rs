use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::Result;
use crate::hash::hash_hex;
use crate::manifest::{self, target::TargetManifest};
use crate::path_guard::resolve_manifest_relative_path;

#[derive(Debug, Clone, Default)]
pub struct VerifyTimings {
    pub stat: Duration,
    pub file_read: Duration,
    pub file_read_bytes: u64,
    pub file_hash: Duration,
    pub file_hash_bytes: u64,
    pub chunk_hash: Duration,
    pub chunk_hash_bytes: u64,
    pub total: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairIssueKind {
    Missing,
    Corrupt,
    Extra,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairIssue {
    pub kind: RepairIssueKind,
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct RepairClassification {
    pub missing_files: Vec<RepairIssue>,
    pub corrupt_files: Vec<RepairIssue>,
    pub extra_files: Vec<RepairIssue>,
}

impl RepairClassification {
    fn push(&mut self, issue: RepairIssue) {
        match issue.kind {
            RepairIssueKind::Missing => self.missing_files.push(issue),
            RepairIssueKind::Corrupt => self.corrupt_files.push(issue),
            RepairIssueKind::Extra => self.extra_files.push(issue),
        }
    }

    pub fn into_messages(self) -> Vec<String> {
        let mut messages = Vec::with_capacity(
            self.missing_files.len() + self.corrupt_files.len() + self.extra_files.len(),
        );
        messages.extend(self.missing_files.into_iter().map(|issue| issue.detail));
        messages.extend(self.corrupt_files.into_iter().map(|issue| issue.detail));
        messages.extend(self.extra_files.into_iter().map(|issue| issue.detail));
        messages
    }
}

pub fn verify_directory(target_manifest: &TargetManifest, dir: &Path) -> Result<Vec<String>> {
    let (errors, _) = verify_directory_with_timings(target_manifest, dir)?;
    Ok(errors)
}

pub fn verify_directory_with_timings(
    target_manifest: &TargetManifest,
    dir: &Path,
) -> Result<(Vec<String>, VerifyTimings)> {
    let (classification, timings) = classify_directory_with_timings(target_manifest, dir)?;
    Ok((classification.into_messages(), timings))
}

pub fn classify_directory(
    target_manifest: &TargetManifest,
    dir: &Path,
) -> Result<RepairClassification> {
    let (classification, _) = classify_directory_with_timings(target_manifest, dir)?;
    Ok(classification)
}

pub fn classify_directory_with_timings(
    target_manifest: &TargetManifest,
    dir: &Path,
) -> Result<(RepairClassification, VerifyTimings)> {
    let total_start = Instant::now();
    let mut classification = RepairClassification::default();
    let mut timings = VerifyTimings::default();
    let manifest_paths: HashSet<&str> = target_manifest
        .files
        .iter()
        .map(|fm| fm.path.as_str())
        .collect();

    for fm in &target_manifest.files {
        let file_path = resolve_manifest_relative_path(dir, &fm.path)?;

        if !file_path.exists() {
            classification.push(RepairIssue {
                kind: RepairIssueKind::Missing,
                path: fm.path.clone(),
                detail: format!("missing file: {}", fm.path),
            });
            continue;
        }

        let t = Instant::now();
        let metadata = match std::fs::metadata(&file_path) {
            Ok(m) => m,
            Err(e) => {
                classification.push(RepairIssue {
                    kind: RepairIssueKind::Corrupt,
                    path: fm.path.clone(),
                    detail: format!("cannot stat {}: {}", fm.path, e),
                });
                continue;
            }
        };
        timings.stat += t.elapsed();

        if metadata.len() != fm.size {
            classification.push(RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: fm.path.clone(),
                detail: format!(
                    "size mismatch for {}: expected {}, got {}",
                    fm.path,
                    fm.size,
                    metadata.len()
                ),
            });
            continue;
        }

        let t = Instant::now();
        let file_data = match std::fs::read(&file_path) {
            Ok(d) => d,
            Err(e) => {
                classification.push(RepairIssue {
                    kind: RepairIssueKind::Corrupt,
                    path: fm.path.clone(),
                    detail: format!("cannot read {}: {}", fm.path, e),
                });
                continue;
            }
        };
        timings.file_read += t.elapsed();
        timings.file_read_bytes += file_data.len() as u64;

        let t = Instant::now();
        let actual_hash = hash_hex(&crate::hash::hash_chunk(&file_data));
        timings.file_hash += t.elapsed();
        timings.file_hash_bytes += file_data.len() as u64;
        if actual_hash != fm.full_file_hash {
            classification.push(RepairIssue {
                kind: RepairIssueKind::Corrupt,
                path: fm.path.clone(),
                detail: format!(
                    "hash mismatch for {}: expected {}, got {}",
                    fm.path, fm.full_file_hash, actual_hash
                ),
            });
        }

        let mut chunk_offset = 0usize;
        for chunk_ref in &fm.chunks {
            let start = chunk_offset;
            let end = start + chunk_ref.raw_size as usize;
            chunk_offset = end;
            if end > file_data.len() {
                classification.push(RepairIssue {
                    kind: RepairIssueKind::Corrupt,
                    path: fm.path.clone(),
                    detail: format!(
                        "chunk reference out of bounds in {}: offset {}, size {}",
                        fm.path, start, chunk_ref.raw_size
                    ),
                });
                continue;
            }

            let chunk_actual = &file_data[start..end];
            let t = Instant::now();
            let chunk_hash_actual = hash_hex(&crate::hash::hash_chunk(chunk_actual));
            timings.chunk_hash += t.elapsed();
            timings.chunk_hash_bytes += chunk_actual.len() as u64;
            if chunk_hash_actual != chunk_ref.chunk_hash {
                classification.push(RepairIssue {
                    kind: RepairIssueKind::Corrupt,
                    path: fm.path.clone(),
                    detail: format!(
                        "chunk hash mismatch in {} at offset {}: expected {}, got {}",
                        fm.path, start, chunk_ref.chunk_hash, chunk_hash_actual
                    ),
                });
            }
        }
    }

    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry?;
        if !entry.path().is_file() {
            continue;
        }

        let Ok(relative_path) = entry.path().strip_prefix(dir) else {
            continue;
        };
        let relative = relative_path.to_string_lossy().replace('\\', "/");
        if manifest_paths.contains(relative.as_str()) {
            continue;
        }

        classification.push(RepairIssue {
            kind: RepairIssueKind::Extra,
            path: relative.clone(),
            detail: format!("extra file: {}", relative),
        });
    }

    timings.total = total_start.elapsed();
    Ok((classification, timings))
}

pub fn estimate_build(
    target_manifest: &TargetManifest,
    available_chunks: &HashSet<String>,
    available_disk_space: u64,
) -> Result<manifest::Estimate> {
    let mut missing_chunks: HashSet<String> = HashSet::new();
    let mut missing_logical: u64 = 0;

    for fm in &target_manifest.files {
        for cr in &fm.chunks {
            if !available_chunks.contains(&cr.chunk_hash) {
                missing_chunks.insert(cr.chunk_hash.clone());
                missing_logical += cr.raw_size;
            }
        }
    }

    let estimate_ratio = 0.5;
    let missing_compressed = (missing_logical as f64 * estimate_ratio) as u64;

    let bundle_estimate = if missing_chunks.is_empty() {
        0
    } else {
        1 + (missing_chunks.len() as u64 / 2000)
    };

    Ok(manifest::Estimate {
        missing_logical_bytes: missing_logical,
        missing_compressed_bytes: missing_compressed,
        missing_chunk_count: missing_chunks.len() as u64,
        required_bundle_count: bundle_estimate,
        range_request_count: missing_chunks.len() as u64,
        local_cache_hits: 0,
        available_disk_space,
        fragmentation_score: bundle_estimate as f64,
    })
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;
