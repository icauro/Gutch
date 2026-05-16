use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::Result;
use crate::hash::hash_hex;
use crate::manifest::{self, TargetManifest};

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

    fn into_messages(self) -> Vec<String> {
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
        let file_path = dir.join(&fm.path);

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
            let chunk_hash_actual = hash_hex(&crate::hash::hash_chunk(chunk_actual));
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
mod tests {
    use super::*;
    use crate::manifest::{TargetManifest, TargetTagSet};

    fn empty_manifest() -> TargetManifest {
        TargetManifest::new(
            "p1".into(),
            "r1".into(),
            "t1".into(),
            TargetTagSet::default(),
        )
    }

    #[test]
    fn estimate_build_no_missing_chunks() {
        let manifest = empty_manifest();
        let available = HashSet::new();
        let result = estimate_build(&manifest, &available, 1_000_000).unwrap();
        assert_eq!(result.missing_chunk_count, 0);
        assert_eq!(result.missing_logical_bytes, 0);
        assert_eq!(result.required_bundle_count, 0);
    }

    #[test]
    fn estimate_build_missing_chunks_counted() {
        let mut manifest = empty_manifest();
        manifest.files.push(crate::manifest::FileManifest {
            path: "game.exe".into(),
            size: 1000,
            mode: 0o755,
            executable: true,
            symlink_target: None,
            full_file_hash: "hash1".into(),
            chunks: vec![
                crate::manifest::ChunkRef {
                    chunk_hash: "a".into(),
                    raw_size: 500,
                },
                crate::manifest::ChunkRef {
                    chunk_hash: "b".into(),
                    raw_size: 500,
                },
            ],
        });
        let available = HashSet::from(["a".into()]);
        let result = estimate_build(&manifest, &available, 1_000_000).unwrap();
        assert_eq!(result.missing_chunk_count, 1);
        assert_eq!(result.missing_logical_bytes, 500);
    }

    #[test]
    fn estimate_build_all_chunks_available() {
        let mut manifest = empty_manifest();
        manifest.files.push(crate::manifest::FileManifest {
            path: "data.bin".into(),
            size: 200,
            mode: 0o644,
            executable: false,
            symlink_target: None,
            full_file_hash: "hash2".into(),
            chunks: vec![
                crate::manifest::ChunkRef {
                    chunk_hash: "x".into(),
                    raw_size: 100,
                },
                crate::manifest::ChunkRef {
                    chunk_hash: "y".into(),
                    raw_size: 100,
                },
            ],
        });
        let available = HashSet::from(["x".into(), "y".into()]);
        let result = estimate_build(&manifest, &available, 500_000).unwrap();
        assert_eq!(result.missing_chunk_count, 0);
        assert_eq!(result.local_cache_hits, 0);
    }

    #[test]
    fn estimate_build_available_disk_space_passed_through() {
        let manifest = empty_manifest();
        let available = HashSet::new();
        let result = estimate_build(&manifest, &available, 42_000).unwrap();
        assert_eq!(result.available_disk_space, 42_000);
    }

    #[test]
    fn verify_contiguous_chunks_detects_corruption() {
        let dir = std::env::temp_dir().join("gutch_verify_contiguous_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let chunk_a_data = b"AAAAA";
        let chunk_b_data = b"BBBBB";
        let mut full_file = Vec::new();
        full_file.extend_from_slice(chunk_a_data);
        full_file.extend_from_slice(chunk_b_data);
        let file_hash = crate::hash::hash_hex(&crate::hash::hash_chunk(&full_file));

        std::fs::write(dir.join("data.bin"), &full_file).unwrap();

        let mut manifest = empty_manifest();
        manifest.files.push(crate::manifest::FileManifest {
            path: "data.bin".into(),
            size: full_file.len() as u64,
            mode: 0o644,
            executable: false,
            symlink_target: None,
            full_file_hash: file_hash,
            chunks: vec![
                crate::manifest::ChunkRef {
                    chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(chunk_a_data)),
                    raw_size: chunk_a_data.len() as u64,
                },
                crate::manifest::ChunkRef {
                    chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(chunk_b_data)),
                    raw_size: chunk_b_data.len() as u64,
                },
            ],
        });
        let errors = verify_directory(&manifest, &dir).unwrap();
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);

        // Now corrupt the file — swap data
        let mut corrupted = Vec::new();
        corrupted.extend_from_slice(chunk_b_data);
        corrupted.extend_from_slice(chunk_a_data);
        std::fs::write(dir.join("data.bin"), &corrupted).unwrap();
        let errors = verify_directory(&manifest, &dir).unwrap();
        assert!(!errors.is_empty(), "expected corruption errors");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_contiguous_chunks_single_chunk() {
        let dir = std::env::temp_dir().join("gutch_verify_single_chunk");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let data = b"single chunk content here";
        let file_hash = crate::hash::hash_hex(&crate::hash::hash_chunk(data));
        std::fs::write(dir.join("single.bin"), data).unwrap();

        let mut manifest = empty_manifest();
        manifest.files.push(crate::manifest::FileManifest {
            path: "single.bin".into(),
            size: data.len() as u64,
            mode: 0o644,
            executable: false,
            symlink_target: None,
            full_file_hash: file_hash,
            chunks: vec![crate::manifest::ChunkRef {
                chunk_hash: crate::hash::hash_hex(&crate::hash::hash_chunk(data)),
                raw_size: data.len() as u64,
            }],
        });
        let errors = verify_directory(&manifest, &dir).unwrap();
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn estimate_build_empty_files_no_chunks() {
        let manifest = empty_manifest();
        let available = HashSet::new();
        let result = estimate_build(&manifest, &available, 0).unwrap();
        assert_eq!(result.missing_chunk_count, 0);
        assert_eq!(result.required_bundle_count, 0);
    }
}
